//! Song analysis for smart shuffle (CONCEPT.md §7), off until the user
//! chooses ("analysis-mode"):
//! - "background": one song at a time on a nice-19 thread, waiting while
//!   on battery;
//! - "full": as fast as possible on several cores; when everything is
//!   done it switches to "background" for songs added later.
//!
//! Pausable, resumes after restarts (results are in the database).

use std::cell::{Cell, RefCell};
use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use gatefold_core::analysis;
use gatefold_core::db::Database;
use gtk::glib::subclass::Signal;
use gtk::prelude::*;
use gtk::subclass::prelude::*;
use gtk::{gio, glib};

use crate::config;

/// Songs fetched from the database per round.
const BATCH: usize = 16;

pub enum Update {
    Progress {
        analyzed: u32,
        total: u32,
    },
    /// Nothing left to analyze.
    Idle,
    /// A full ("as fast as possible") run found nothing left.
    FullDone,
}

/// Shared by the main thread and the analysis threads.
#[derive(Debug, Default)]
pub struct Control {
    /// Wait: analysis off, paused, or on battery in the background mode.
    hold: AtomicBool,
    /// "full": extra threads, no pauses between songs.
    fast: AtomicBool,
    /// Songs a thread is working on, so threads don't take the same one.
    claimed: Mutex<HashSet<i64>>,
}

mod imp {
    use std::sync::OnceLock;

    use super::*;

    #[derive(Debug, Default, glib::Properties)]
    #[properties(wrapper_type = super::Analyzer)]
    pub struct Analyzer {
        #[property(get)]
        pub analyzed: Cell<u32>,
        #[property(get)]
        pub total: Cell<u32>,
        /// Working through songs right now.
        #[property(get)]
        pub busy: Cell<bool>,
        /// Paused by the user (Preferences).
        #[property(get, set = Self::set_paused)]
        pub paused: Cell<bool>,
        /// Running on battery (UPower): analysis waits.
        #[property(get)]
        pub on_battery: Cell<bool>,
        /// Read by the analysis threads between songs.
        pub control: Arc<Control>,
        pub wake: RefCell<Option<std::sync::mpsc::Sender<()>>>,
        /// Extra threads of a full run that are still working.
        pub extra_lanes: Arc<std::sync::atomic::AtomicUsize>,
        pub updates: RefCell<Option<async_channel::Sender<Update>>>,
        pub upower: RefCell<Option<gio::DBusProxy>>,
        pub settings: OnceLock<gio::Settings>,
    }

    impl Analyzer {
        fn set_paused(&self, paused: bool) {
            self.paused.set(paused);
            self.obj().update_hold();
        }
    }

    #[glib::object_subclass]
    impl ObjectSubclass for Analyzer {
        const NAME: &'static str = "GatefoldAnalyzer";
        type Type = super::Analyzer;
    }

    #[glib::derived_properties]
    impl ObjectImpl for Analyzer {
        fn signals() -> &'static [Signal] {
            static SIGNALS: OnceLock<Vec<Signal>> = OnceLock::new();
            // New results are in (smart shuffle reloads its data).
            SIGNALS.get_or_init(|| vec![Signal::builder("results-changed").build()])
        }
    }
}

glib::wrapper! {
    pub struct Analyzer(ObjectSubclass<imp::Analyzer>);
}

impl Analyzer {
    pub fn new() -> Self {
        glib::Object::new()
    }

    fn settings(&self) -> &gio::Settings {
        self.imp()
            .settings
            .get_or_init(|| gio::Settings::new(config::APP_ID))
    }

    pub fn start(&self) {
        let (wake, woken) = std::sync::mpsc::channel::<()>();
        let (updates, received) = async_channel::unbounded::<Update>();
        self.imp().wake.replace(Some(wake));
        self.imp().updates.replace(Some(updates.clone()));
        let control = self.imp().control.clone();
        std::thread::Builder::new()
            .name("analysis".into())
            .spawn(move || worker(&control, &woken, &updates))
            .expect("can start a thread");

        glib::spawn_future_local(glib::clone!(
            #[weak(rename_to = analyzer)]
            self,
            async move {
                let mut since_signal = 0;
                while let Ok(update) = received.recv().await {
                    let imp = analyzer.imp();
                    match update {
                        Update::Progress { analyzed, total } => {
                            imp.analyzed.set(analyzed);
                            imp.total.set(total);
                            analyzer.notify_analyzed();
                            analyzer.notify_total();
                            if !imp.busy.replace(true) {
                                analyzer.notify_busy();
                            }
                            since_signal += 1;
                            // Smart shuffle reloads its data now and then.
                            if since_signal >= 25 {
                                since_signal = 0;
                                analyzer.emit_by_name::<()>("results-changed", &[]);
                            }
                        }
                        Update::Idle => {
                            if imp.busy.replace(false) {
                                analyzer.notify_busy();
                            }
                            since_signal = 0;
                            analyzer.emit_by_name::<()>("results-changed", &[]);
                        }
                        Update::FullDone => {
                            // Done: new songs from now on in the background.
                            if analyzer.settings().string("analysis-mode") == "full" {
                                let _ = analyzer
                                    .settings()
                                    .set_string("analysis-mode", "background");
                            }
                        }
                    }
                }
            }
        ));

        self.settings().connect_changed(
            Some("analysis-mode"),
            glib::clone!(
                #[weak(rename_to = analyzer)]
                self,
                move |_, _| analyzer.update_hold()
            ),
        );
        self.watch_battery();
        self.update_hold();
    }

    /// Looks again for songs to analyze (after a scan).
    pub fn wake(&self) {
        if let Some(wake) = self.imp().wake.borrow().as_ref() {
            let _ = wake.send(());
        }
    }

    /// Off, background or full, from the setting, pause and battery.
    fn update_hold(&self) {
        let imp = self.imp();
        let mode = self.settings().string("analysis-mode");
        let hold =
            mode == "off" || imp.paused.get() || (mode == "background" && imp.on_battery.get());
        let fast = mode == "full";
        imp.control.hold.store(hold, Ordering::SeqCst);
        imp.control.fast.store(fast, Ordering::SeqCst);
        if hold {
            return;
        }
        if fast {
            self.start_extra_lanes();
        }
        self.wake();
    }

    /// A full run: more threads, one per spare core.
    fn start_extra_lanes(&self) {
        let imp = self.imp();
        let cores = std::thread::available_parallelism().map_or(2, |n| n.get());
        let wanted = cores.saturating_sub(2).clamp(1, 6);
        let Some(updates) = imp.updates.borrow().clone() else {
            return;
        };
        while imp.extra_lanes.load(Ordering::SeqCst) < wanted {
            imp.extra_lanes.fetch_add(1, Ordering::SeqCst);
            let control = imp.control.clone();
            let lanes = imp.extra_lanes.clone();
            let updates = updates.clone();
            let started = std::thread::Builder::new()
                .name("analysis-extra".into())
                .spawn(move || {
                    extra_lane(&control, &updates);
                    lanes.fetch_sub(1, Ordering::SeqCst);
                });
            if started.is_err() {
                imp.extra_lanes.fetch_sub(1, Ordering::SeqCst);
                break;
            }
        }
    }

    /// UPower's OnBattery, on the system bus; without UPower: not on battery.
    fn watch_battery(&self) {
        gio::DBusProxy::for_bus(
            gio::BusType::System,
            gio::DBusProxyFlags::DO_NOT_AUTO_START,
            None,
            "org.freedesktop.UPower",
            "/org/freedesktop/UPower",
            "org.freedesktop.UPower",
            gio::Cancellable::NONE,
            glib::clone!(
                #[weak(rename_to = analyzer)]
                self,
                move |result| {
                    let Ok(proxy) = result else { return };
                    let update = glib::clone!(
                        #[weak]
                        analyzer,
                        move |proxy: &gio::DBusProxy| {
                            let on_battery = proxy
                                .cached_property("OnBattery")
                                .and_then(|v| v.get::<bool>())
                                .unwrap_or(false);
                            if analyzer.imp().on_battery.replace(on_battery) != on_battery {
                                analyzer.notify_on_battery();
                                analyzer.update_hold();
                            }
                        }
                    );
                    update(&proxy);
                    proxy.connect_local("g-properties-changed", false, move |values| {
                        if let Ok(proxy) = values[0].get::<gio::DBusProxy>() {
                            update(&proxy);
                        }
                        None
                    });
                    analyzer.imp().upower.replace(Some(proxy));
                }
            ),
        );
    }
}

impl Default for Analyzer {
    fn default() -> Self {
        Self::new()
    }
}

/// The main analysis thread: analyzes what's missing, then sleeps until
/// woken.
fn worker(
    control: &Control,
    woken: &std::sync::mpsc::Receiver<()>,
    updates: &async_channel::Sender<Update>,
) {
    set_priority(19);
    let Some(mut db) = open_db() else { return };
    progress(&db, updates);
    loop {
        if control.hold.load(Ordering::SeqCst) {
            let _ = updates.send_blocking(Update::Idle);
            if woken.recv().is_err() {
                return;
            }
            continue;
        }
        if !work_through_batch(&mut db, control, updates) {
            progress(&db, updates);
            if control.fast.load(Ordering::SeqCst) {
                let _ = updates.send_blocking(Update::FullDone);
            }
            let _ = updates.send_blocking(Update::Idle);
            if woken.recv().is_err() {
                return;
            }
            // Several wakes may have piled up.
            while woken.try_recv().is_ok() {}
        }
    }
}

/// An extra thread of a full run: works until there's nothing left or the
/// run ends.
fn extra_lane(control: &Control, updates: &async_channel::Sender<Update>) {
    set_priority(5);
    let Some(mut db) = open_db() else { return };
    while control.fast.load(Ordering::SeqCst) && !control.hold.load(Ordering::SeqCst) {
        if !work_through_batch(&mut db, control, updates) {
            return;
        }
    }
}

/// Analyzes one batch of songs; false if there was nothing to do.
fn work_through_batch(
    db: &mut Database,
    control: &Control,
    updates: &async_channel::Sender<Update>,
) -> bool {
    let batch = db.tracks_to_analyze(BATCH * 4).unwrap_or_default();
    let mut did_any = false;
    for (track_id, path) in batch {
        if control.hold.load(Ordering::SeqCst) {
            return true;
        }
        // Another thread may be on it already.
        if !control.claimed.lock().is_ok_and(|mut c| c.insert(track_id)) {
            continue;
        }
        did_any = true;
        analyze_one(db, track_id, &path);
        if let Ok(mut claimed) = control.claimed.lock() {
            claimed.remove(&track_id);
        }
        progress(db, updates);
        // In the background, leave the CPU some air between songs.
        if !control.fast.load(Ordering::SeqCst) {
            std::thread::sleep(analysis::PAUSE_BETWEEN);
        }
    }
    did_any
}

fn open_db() -> Option<Database> {
    Database::open(&config::database_path())
        .map_err(|err| tracing::error!("Analysis can't open the database: {err}"))
        .ok()
}

fn progress(db: &Database, updates: &async_channel::Sender<Update>) {
    if let Ok((analyzed, total)) = db.analysis_progress() {
        let _ = updates.send_blocking(Update::Progress { analyzed, total });
    }
}

fn analyze_one(db: &mut Database, track_id: i64, path: &std::path::Path) {
    // A file that can't be read gets a key of its own, so it's marked as
    // failed and not tried again until it changes.
    let key = analysis::content_key(path).unwrap_or_else(|err| {
        tracing::debug!("Cannot read {}: {err}", path.display());
        format!("unreadable:{track_id}")
    });
    let known = db.has_analysis(&key).unwrap_or(false);
    if !known {
        let result = if key.starts_with("unreadable:") {
            Err("unreadable".to_string())
        } else {
            analysis::analyze(path)
        };
        if let Err(err) = &result {
            tracing::info!("Cannot analyze {}: {err}", path.display());
        }
        if let Err(err) = db.save_analysis(&key, result.as_ref().ok(), crate::metadata::now()) {
            tracing::warn!("Cannot save the analysis: {err}");
            return;
        }
    }
    if let Err(err) = db.set_content_key(track_id, &key) {
        tracing::warn!("Cannot save the content key: {err}");
    }
}

/// Sets the calling thread's CPU priority ("nice": 19 is the lowest).
fn set_priority(nice: i32) {
    // SAFETY: plain syscalls on the calling thread; no memory is shared.
    unsafe {
        let tid = libc::syscall(libc::SYS_gettid) as libc::id_t;
        libc::setpriority(libc::PRIO_PROCESS, tid, nice);
    }
}
