//! The background analysis for smart shuffle (CONCEPT.md §7): one song at
//! a time on a low-priority thread, pausable, paused on battery, resuming
//! after restarts (results are in the database). Progress for Preferences.

use std::cell::{Cell, RefCell};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use gatefold_core::analysis;
use gatefold_core::db::Database;
use gtk::glib::subclass::Signal;
use gtk::prelude::*;
use gtk::subclass::prelude::*;
use gtk::{gio, glib};

use crate::config;

/// Songs fetched from the database per round.
const BATCH: usize = 16;

enum Update {
    Progress {
        analyzed: u32,
        total: u32,
    },
    /// Nothing left to analyze.
    Idle,
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
        /// Read by the worker between songs.
        pub hold: Arc<AtomicBool>,
        pub wake: RefCell<Option<std::sync::mpsc::Sender<()>>>,
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
        let hold = self.imp().hold.clone();
        std::thread::Builder::new()
            .name("analysis".into())
            .spawn(move || worker(&hold, &woken, &updates))
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
                    }
                }
            }
        ));

        self.settings().connect_changed(
            Some("analysis-enabled"),
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

    /// Whether the worker should wait: off, paused, or on battery.
    fn update_hold(&self) {
        let imp = self.imp();
        let hold = !self.settings().boolean("analysis-enabled")
            || imp.paused.get()
            || imp.on_battery.get();
        imp.hold.store(hold, Ordering::SeqCst);
        if !hold {
            self.wake();
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

/// The worker thread: analyzes what's missing, then sleeps until woken.
fn worker(
    hold: &AtomicBool,
    woken: &std::sync::mpsc::Receiver<()>,
    updates: &async_channel::Sender<Update>,
) {
    lower_priority();
    let mut db = match Database::open(&config::database_path()) {
        Ok(db) => db,
        Err(err) => {
            tracing::error!("Analysis can't open the database: {err}");
            return;
        }
    };
    let progress = |db: &Database| {
        if let Ok((analyzed, total)) = db.analysis_progress() {
            let _ = updates.send_blocking(Update::Progress { analyzed, total });
        }
    };
    progress(&db);
    loop {
        if hold.load(Ordering::SeqCst) {
            let _ = updates.send_blocking(Update::Idle);
            if woken.recv().is_err() {
                return;
            }
            continue;
        }
        let batch = db.tracks_to_analyze(BATCH).unwrap_or_default();
        if batch.is_empty() {
            progress(&db);
            let _ = updates.send_blocking(Update::Idle);
            if woken.recv().is_err() {
                return;
            }
            // Several wakes may have piled up.
            while woken.try_recv().is_ok() {}
            continue;
        }
        for (track_id, path) in batch {
            if hold.load(Ordering::SeqCst) {
                break;
            }
            analyze_one(&mut db, track_id, &path);
            progress(&db);
            std::thread::sleep(analysis::PAUSE_BETWEEN);
        }
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

/// The analysis thread runs at the lowest CPU priority ("nice 19").
fn lower_priority() {
    // SAFETY: plain syscalls on the calling thread; no memory is shared.
    unsafe {
        let tid = libc::syscall(libc::SYS_gettid) as libc::id_t;
        libc::setpriority(libc::PRIO_PROCESS, tid, 19);
    }
}
