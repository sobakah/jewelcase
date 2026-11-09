//! Companion tools (CONCEPT.md §10): lyrembed, cdport and the tag editor.
//!
//! Tools run as `gio::Subprocess`es with their output read on the main
//! loop. A tool with a page (lyrembed's editor, cdport) prints its local
//! address, which opens in the default browser; a status bar offers
//! "Show Page" and "Stop" (SIGINT, as Ctrl+C). Results arrive through the
//! files: the library's inotify watcher picks them up.
//!
//! cdport 1.3.0 and lyrembed 1.2.0 and newer are reached over D-Bus when
//! their service is set up (`--install-dbus-service`): their runs then
//! belong to them, not to Gatefold, and it's the only way from a Flatpak.

use std::cell::{Cell, OnceCell, RefCell};
use std::collections::VecDeque;
use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::Duration;

use adw::prelude::*;
use gatefold_tools::editor::{self, Editor};
use gatefold_tools::lyrembed::AutoResult;
use gatefold_tools::{Version, cdport, find_program, lyrembed, parse_version};
use gettextrs::gettext;
use gtk::glib::subclass::Signal;
use gtk::subclass::prelude::*;
use gtk::{gio, glib};

use crate::application::GatefoldApplication;
use crate::config::APP_ID;
use crate::library_folders;

/// SIGINT: the tools stop as with Ctrl+C (cdport finishes cyanrip's log).
const SIGINT: i32 = 2;
/// cdport's Import and lyrembed's OpenPage answer once the page is up,
/// within 20 s.
const START_TIMEOUT_MS: i32 = 30_000;

/// A found tool: its program and version.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolInfo {
    pub path: PathBuf,
    pub version: Option<Version>,
}

/// What detection found (runs on a background thread).
#[derive(Debug, Clone, Default)]
pub struct Detected {
    pub lyrembed: Lyrembed,
    pub cdport: Cdport,
    /// The lyrembed program has `romanize --stdio` (without D-Bus).
    pub romanize_stdio: bool,
    pub ear_tag: bool,
}

impl Detected {
    /// Automatic romanization for display is possible (§5.1).
    pub fn can_romanize(&self) -> bool {
        self.lyrembed.dbus || (self.lyrembed.program.is_some() && self.romanize_stdio)
    }
}

/// A companion tool as found: over D-Bus, as a program here, or both.
#[derive(Debug, Clone)]
pub struct Companion<C> {
    /// The program. A Flatpak doesn't see it.
    pub program: Option<ToolInfo>,
    /// Its D-Bus service answered.
    pub dbus: bool,
    /// Why its D-Bus service didn't start, when it's set up but broken
    /// (e.g. the program was removed or moved since).
    pub dbus_error: Option<String>,
    pub version: Option<Version>,
    pub config: C,
    /// The first version with the D-Bus interface.
    pub dbus_version: Version,
    /// The program's old name (cdport's `cdimport`).
    pub old_name: Option<&'static str>,
}

/// cdport (`cdport`, or `cdimport` before 1.3.0).
pub type Cdport = Companion<cdport::Config>;
pub type Lyrembed = Companion<lyrembed::Config>;

impl Default for Cdport {
    fn default() -> Self {
        Companion::new(cdport::DBUS_VERSION, Some("cdimport"))
    }
}

impl Default for Lyrembed {
    fn default() -> Self {
        Companion::new(lyrembed::DBUS_VERSION, None)
    }
}

impl<C: Default> Companion<C> {
    fn new(dbus_version: Version, old_name: Option<&'static str>) -> Self {
        Self {
            program: None,
            dbus: false,
            dbus_error: None,
            version: None,
            config: C::default(),
            dbus_version,
            old_name,
        }
    }
}

impl<C> Companion<C> {
    /// Gatefold can use it.
    pub fn usable(&self) -> bool {
        self.dbus || self.program.is_some()
    }

    /// A version from before the D-Bus interface (for cdport: cdimport).
    pub fn is_old(&self) -> bool {
        let Some(program) = self.program.as_ref().filter(|_| !self.dbus) else {
            return false;
        };
        match self.version {
            Some(version) => version < self.dbus_version,
            // Its version unknown: the name tells.
            None => self
                .old_name
                .is_some_and(|old| program.path.file_name().is_some_and(|n| n == old)),
        }
    }

    /// A version with the D-Bus interface, without its service set up.
    pub fn can_set_up_service(&self) -> bool {
        !self.dbus && self.program.is_some() && !self.is_old()
    }
}

/// The companion tools with a page (and a D-Bus interface).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PageTool {
    Lyrembed,
    Cdport,
}

impl PageTool {
    pub fn name(self) -> &'static str {
        match self {
            Self::Lyrembed => "lyrembed",
            Self::Cdport => "cdport",
        }
    }

    fn bus_name(self) -> &'static str {
        match self {
            Self::Lyrembed => lyrembed::BUS_NAME,
            Self::Cdport => cdport::BUS_NAME,
        }
    }

    fn object_path(self) -> &'static str {
        match self {
            Self::Lyrembed => lyrembed::OBJECT_PATH,
            Self::Cdport => cdport::OBJECT_PATH,
        }
    }

    fn interface(self) -> &'static str {
        match self {
            Self::Lyrembed => lyrembed::INTERFACE,
            Self::Cdport => cdport::INTERFACE,
        }
    }

    fn status(self) -> String {
        match self {
            Self::Lyrembed => gettext("lyrembed is open in your browser"),
            Self::Cdport => gettext("Importing a CD with cdport"),
        }
    }
}

/// A tool with a page, while it runs.
#[derive(Debug, Clone)]
pub enum PageRun {
    /// Started by Gatefold: it ends with Gatefold.
    Process(gio::Subprocess),
    /// A run of the tool's D-Bus service (its ID; empty until the service
    /// answered): it runs on without Gatefold.
    Service(String),
}

/// A page to open (now, or once the running one has closed).
#[derive(Debug, Clone)]
pub enum PageStart {
    Command {
        tool: PageTool,
        argv: Vec<OsString>,
        status: String,
    },
    /// Over D-Bus: cdport (`folder` continues an unfinished import) or
    /// lyrembed (`folder`'s page).
    Service {
        tool: PageTool,
        folder: Option<PathBuf>,
    },
}

mod imp {
    use super::*;

    #[derive(Debug, Default, glib::Properties)]
    #[properties(wrapper_type = super::Tools)]
    pub struct Tools {
        /// The status bar text while a tool page is open; empty otherwise.
        #[property(get)]
        pub page_status: RefCell<String>,
        /// An audio CD is in a drive.
        #[property(get)]
        pub cd_present: Cell<bool>,
        /// Album folders with an unfinished cdport run.
        #[property(get)]
        pub unfinished_imports: Cell<u32>,
        /// lyrembed --auto jobs running or waiting.
        #[property(get)]
        pub lyrics_jobs: Cell<u32>,

        pub detected: RefCell<Detected>,
        /// Counts detections; an older one finishing later is dropped.
        pub detect_generation: Cell<u64>,
        pub page_run: RefCell<Option<PageRun>>,
        pub page_tool: Cell<Option<PageTool>>,
        pub page_url: RefCell<Option<String>>,
        /// Counts the pages opened, to tell them apart later.
        pub page_generation: Cell<u64>,
        /// Stop was asked for before the service's run started.
        pub stop_when_started: Cell<bool>,
        /// A page to open once the running one has closed.
        pub pending_page: RefCell<Option<PageStart>>,
        /// The session bus (the tools' D-Bus interfaces).
        pub bus: OnceCell<gio::DBusConnection>,
        pub finished_signals: RefCell<Vec<gio::SignalSubscription>>,
        pub auto_queue: RefCell<VecDeque<PathBuf>>,
        pub auto_running: Cell<bool>,
        /// The lyrics search running over D-Bus (lyrembed's run ID).
        pub auto_run: RefCell<Option<String>>,
        pub unfinished: RefCell<Vec<PathBuf>>,
        /// When the last cdport run started (Unix seconds), for the
        /// "New album" toast.
        pub import_started: Cell<Option<i64>>,
        pub volume_monitor: OnceCell<gio::VolumeMonitor>,
        pub settings: OnceCell<gio::Settings>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for Tools {
        const NAME: &'static str = "GatefoldTools";
        type Type = super::Tools;
    }

    #[glib::derived_properties]
    impl ObjectImpl for Tools {
        fn signals() -> &'static [Signal] {
            static SIGNALS: OnceLock<Vec<Signal>> = OnceLock::new();
            SIGNALS.get_or_init(|| {
                vec![
                    // Detection finished (Preferences and menus update).
                    Signal::builder("detected").build(),
                    // A message for the user (toast).
                    Signal::builder("message")
                        .param_types([String::static_type()])
                        .build(),
                    // A tool is needed but not set up (its name): the
                    // window offers the instructions in Preferences.
                    Signal::builder("missing")
                        .param_types([String::static_type()])
                        .build(),
                ]
            })
        }
    }
}

glib::wrapper! {
    pub struct Tools(ObjectSubclass<imp::Tools>);
}

impl Tools {
    pub fn new() -> Self {
        glib::Object::new()
    }

    pub fn start(&self) {
        let settings = gio::Settings::new(APP_ID);
        for key in ["lyrembed-path", "cdport-path"] {
            settings.connect_changed(
                Some(key),
                glib::clone!(
                    #[weak(rename_to = tools)]
                    self,
                    move |_, _| tools.detect()
                ),
            );
        }
        let _ = self.imp().settings.set(settings);
        self.detect();
        self.watch_cds();
        self.watch_services();
    }

    fn settings(&self) -> &gio::Settings {
        self.imp().settings.get().expect("set in start()")
    }

    pub fn detected(&self) -> Detected {
        self.imp().detected.borrow().clone()
    }

    fn message(&self, text: &str) {
        self.emit_by_name::<()>("message", &[&text]);
    }

    // Detection (§10.0) ------------------------------------------------------

    pub fn detect(&self) {
        let overrides = (
            PathBuf::from(self.settings().string("lyrembed-path").as_str()),
            PathBuf::from(self.settings().string("cdport-path").as_str()),
        );
        let generation = self.imp().detect_generation.get() + 1;
        self.imp().detect_generation.set(generation);
        glib::spawn_future_local(glib::clone!(
            #[weak(rename_to = tools)]
            self,
            async move {
                let detected = gio::spawn_blocking(move || detect(&overrides.0, &overrides.1))
                    .await
                    .unwrap_or_default();
                // A changed path started a newer detection meanwhile.
                if tools.imp().detect_generation.get() != generation {
                    return;
                }
                tracing::info!("Companion tools: {detected:?}");
                let services = [
                    (PageTool::Cdport, detected.cdport.dbus),
                    (PageTool::Lyrembed, detected.lyrembed.dbus),
                ];
                tools.imp().detected.replace(detected);
                tools.check_unfinished_imports();
                for (tool, dbus) in services {
                    if dbus {
                        tools.adopt_service_run(tool);
                    }
                }
                tools.emit_by_name::<()>("detected", &[]);
            }
        ));
    }

    /// The lyrembed program (without D-Bus: `romanize --stdio`).
    pub fn lyrembed_path(&self) -> Option<PathBuf> {
        self.lyrembed().program.map(|t| t.path)
    }

    fn lyrembed(&self) -> Lyrembed {
        self.imp().detected.borrow().lyrembed.clone()
    }

    /// lyrembed can be used (over D-Bus, or as a program).
    pub fn lyrembed_usable(&self) -> bool {
        self.imp().detected.borrow().lyrembed.usable()
    }

    // Running tools ----------------------------------------------------------

    /// Starts a process; `on_line` gets stdout and stderr lines, `on_exit`
    /// the exit code (130 when stopped by a signal).
    fn spawn(
        &self,
        argv: &[OsString],
        on_line: impl Fn(&str) + Clone + 'static,
        on_exit: impl FnOnce(i32) + 'static,
    ) -> Option<gio::Subprocess> {
        let args: Vec<&OsStr> = argv.iter().map(OsString::as_os_str).collect();
        let process = match gio::Subprocess::newv(
            &args,
            gio::SubprocessFlags::STDOUT_PIPE | gio::SubprocessFlags::STDERR_PIPE,
        ) {
            Ok(process) => process,
            Err(err) => {
                tracing::warn!("Cannot start {:?}: {err}", argv.first());
                self.message(&gettext("The tool couldn't be started"));
                return None;
            }
        };
        for pipe in [process.stdout_pipe(), process.stderr_pipe()]
            .into_iter()
            .flatten()
        {
            let on_line = on_line.clone();
            glib::spawn_future_local(async move {
                let stream = gio::DataInputStream::new(&pipe);
                while let Ok(Some(line)) =
                    stream.read_line_utf8_future(glib::Priority::DEFAULT).await
                {
                    tracing::debug!("tool: {line}");
                    on_line(&line);
                }
            });
        }
        let waiting = process.clone();
        glib::spawn_future_local(async move {
            let _ = waiting.wait_future().await;
            let code = if waiting.has_exited() {
                waiting.exit_status()
            } else {
                130
            };
            on_exit(code);
        });
        Some(process)
    }

    /// Opens a tool's page: one at a time (§10.0).
    fn open_page(&self, start: PageStart) {
        let imp = self.imp();
        if imp.page_run.borrow().is_some() {
            // One at a time: offer to close the running one first.
            let open = imp.page_tool.get();
            glib::spawn_future_local(glib::clone!(
                #[weak(rename_to = tools)]
                self,
                async move {
                    if tools.confirm_replace_page(open).await {
                        tools.imp().pending_page.replace(Some(start));
                        tools.stop_page();
                    }
                }
            ));
            return;
        }
        match start {
            PageStart::Command { tool, argv, status } => self.run_page_command(tool, &argv, status),
            PageStart::Service { tool, folder } => self.start_service_run(tool, folder),
        }
    }

    fn run_page_command(&self, tool: PageTool, argv: &[OsString], status: String) {
        let process = self.spawn(
            argv,
            glib::clone!(
                #[weak(rename_to = tools)]
                self,
                move |line| {
                    if let Some(url) = gatefold_tools::page_url(line)
                        && tools.imp().page_url.borrow().is_none()
                    {
                        tools.imp().page_url.replace(Some(url));
                        tools.show_page();
                    }
                }
            ),
            glib::clone!(
                #[weak(rename_to = tools)]
                self,
                move |code| tools.page_closed(tool, code)
            ),
        );
        let Some(process) = process else { return };
        self.page_opened(tool, PageRun::Process(process), None, status);
        if tool == PageTool::Lyrembed {
            // lyrembed's editor plays the track in the page (§10.0).
            GatefoldApplication::get().player().pause();
        }
    }

    fn page_opened(&self, tool: PageTool, run: PageRun, url: Option<String>, status: String) {
        let imp = self.imp();
        imp.page_run.replace(Some(run));
        imp.page_tool.set(Some(tool));
        imp.page_url.replace(url);
        imp.page_generation.set(imp.page_generation.get() + 1);
        imp.stop_when_started.set(false);
        imp.page_status.replace(status);
        self.notify_page_status();
    }

    fn page_closed(&self, tool: PageTool, code: i32) {
        let imp = self.imp();
        imp.page_run.replace(None);
        imp.page_tool.set(None);
        imp.page_url.replace(None);
        imp.page_status.replace(String::new());
        self.notify_page_status();
        tracing::info!("{tool:?} page closed with {code}");
        let library = GatefoldApplication::get().library();
        library.rescan();
        if tool == PageTool::Cdport {
            self.check_unfinished_imports();
        }
        if let Some(start) = imp.pending_page.take() {
            self.open_page(start);
        }
    }

    async fn confirm_replace_page(&self, open: Option<PageTool>) -> bool {
        let name = open.unwrap_or(PageTool::Lyrembed).name();
        let dialog = adw::AlertDialog::builder()
            // Translators: {} is a program name (lyrembed, cdport).
            .heading(gettext("Close the Open {} Page?").replace("{}", name))
            .body(gettext(
                "Only one tool page can be open at a time. Changes you haven't saved there are lost.",
            ))
            .close_response("cancel")
            .default_response("replace")
            .build();
        dialog.add_responses(&[
            ("cancel", &gettext("_Cancel")),
            ("replace", &gettext("_Close and Open New")),
        ]);
        dialog.set_response_appearance("replace", adw::ResponseAppearance::Destructive);
        let window = GatefoldApplication::get().active_window();
        dialog.choose_future(window.as_ref()).await == "replace"
    }

    /// Reopens the page's address (if the tab was lost). lyrembed's
    /// service opens its pages itself (their address isn't handed out).
    pub fn show_page(&self) {
        let imp = self.imp();
        if imp.page_tool.get() == Some(PageTool::Lyrembed)
            && let Some(PageRun::Service(run)) = imp.page_run.borrow().clone()
            && !run.is_empty()
        {
            glib::spawn_future_local(glib::clone!(
                #[weak(rename_to = tools)]
                self,
                async move {
                    let args = glib::Variant::tuple_from_iter([run.to_variant()]);
                    let reply = tools
                        .call_service(
                            PageTool::Lyrembed,
                            "ShowPage",
                            Some(args),
                            Some("(b)"),
                            10_000,
                        )
                        .await;
                    if !matches!(reply.map(|r| r.get::<(bool,)>()), Ok(Some((true,)))) {
                        tools.message(&gettext("lyrembed couldn't open your browser"));
                    }
                }
            ));
            return;
        }
        let Some(url) = imp.page_url.borrow().clone() else {
            return;
        };
        // Development aid for headless tests: never touch the real browser.
        if std::env::var_os("GATEFOLD_NO_BROWSER").is_some() {
            tracing::info!("Tool page (not opened): {url}");
            return;
        }
        let window = GatefoldApplication::get().active_window();
        gtk::UriLauncher::new(&url).launch(window.as_ref(), gio::Cancellable::NONE, |result| {
            if let Err(err) = result {
                tracing::warn!("Cannot open the tool page: {err}");
            }
        });
    }

    /// Stops the tool with a page: SIGINT, never SIGKILL (§10.0), so
    /// cdport can finish cyanrip's log and remember where it was (over
    /// D-Bus, its Stop does the same).
    pub fn stop_page(&self) {
        let imp = self.imp();
        let Some(run) = imp.page_run.borrow().clone() else {
            return;
        };
        match run {
            PageRun::Process(process) => process.send_signal(SIGINT),
            // The service hasn't answered yet: stopped once it has.
            PageRun::Service(id) if id.is_empty() => imp.stop_when_started.set(true),
            PageRun::Service(id) => {
                if let Some(tool) = imp.page_tool.get() {
                    self.stop_service_run(tool, id);
                }
            }
        }
        let generation = imp.page_generation.get();
        glib::timeout_add_seconds_local_once(
            10,
            glib::clone!(
                #[weak(rename_to = tools)]
                self,
                move || {
                    let imp = tools.imp();
                    if imp.page_run.borrow().is_some() && imp.page_generation.get() == generation {
                        tracing::info!("The tool page is still finishing");
                        tools.message(&gettext("The tool is still finishing; it stops by itself"));
                    }
                }
            ),
        );
    }

    /// A tool with a page runs (a CD import, lyrembed's page).
    pub fn page_running(&self) -> bool {
        self.imp().page_run.borrow().is_some()
    }

    /// The running page tool ends when Gatefold quits (one Gatefold
    /// started; runs over D-Bus go on).
    pub fn page_needs_gatefold(&self) -> bool {
        matches!(*self.imp().page_run.borrow(), Some(PageRun::Process(_)))
    }

    // lyrembed (§10.1) -------------------------------------------------------

    /// "Check & Edit in lyrembed…": the album folder's page.
    pub fn lyrembed_page(&self, album_dir: &Path) {
        let lyrembed = self.lyrembed();
        if lyrembed.dbus {
            self.open_page(PageStart::Service {
                tool: PageTool::Lyrembed,
                folder: Some(album_dir.to_path_buf()),
            });
            return;
        }
        let Some(program) = lyrembed.program.map(|p| p.path) else {
            self.missing_tool("lyrembed");
            return;
        };
        self.open_page(PageStart::Command {
            tool: PageTool::Lyrembed,
            argv: lyrembed::page_args(&program, album_dir),
            status: PageTool::Lyrembed.status(),
        });
    }

    /// "Find Lyrics Automatically": `lyrembed --auto` (or its service's
    /// FetchLyrics), one album folder per call, one job at a time.
    pub fn find_lyrics(&self, album_dir: &Path) {
        if !self.lyrembed_usable() {
            self.missing_tool("lyrembed");
            return;
        }
        let imp = self.imp();
        if !imp.auto_queue.borrow().iter().any(|d| d == album_dir) {
            imp.auto_queue
                .borrow_mut()
                .push_back(album_dir.to_path_buf());
        }
        self.update_lyrics_jobs();
        self.run_next_lyrics_job();
    }

    fn update_lyrics_jobs(&self) {
        let imp = self.imp();
        let jobs = imp.auto_queue.borrow().len() as u32 + u32::from(imp.auto_running.get());
        if imp.lyrics_jobs.replace(jobs) != jobs {
            self.notify_lyrics_jobs();
        }
    }

    fn run_next_lyrics_job(&self) {
        let imp = self.imp();
        if imp.auto_running.get() {
            return;
        }
        let Some(dir) = imp.auto_queue.borrow_mut().pop_front() else {
            return;
        };
        let lyrembed = self.lyrembed();
        imp.auto_running.set(true);
        self.update_lyrics_jobs();
        if lyrembed.dbus {
            self.fetch_lyrics_over_dbus(dir, lyrembed.program.map(|p| p.path));
            return;
        }
        let Some(program) = lyrembed.program.map(|p| p.path) else {
            imp.auto_running.set(false);
            self.update_lyrics_jobs();
            return;
        };
        self.fetch_lyrics_with(&program, &dir);
    }

    /// `lyrembed <dir> --auto`.
    fn fetch_lyrics_with(&self, program: &Path, dir: &Path) {
        let imp = self.imp();
        let process = self.spawn(
            &lyrembed::auto_args(program, dir),
            |_| {},
            glib::clone!(
                #[weak(rename_to = tools)]
                self,
                move |code| {
                    tools.imp().auto_running.set(false);
                    tools.update_lyrics_jobs();
                    tools.lyrics_job_done(code);
                    tools.run_next_lyrics_job();
                }
            ),
        );
        if process.is_none() {
            imp.auto_running.set(false);
            self.update_lyrics_jobs();
            self.run_next_lyrics_job();
        }
    }

    /// lyrembed's FetchLyrics. It only searches inside the music folder;
    /// elsewhere the program does it, when Gatefold sees it.
    fn fetch_lyrics_over_dbus(&self, dir: PathBuf, program: Option<PathBuf>) {
        let options = glib::VariantDict::new(None).end();
        let args =
            glib::Variant::tuple_from_iter([dir.to_string_lossy().as_ref().to_variant(), options]);
        glib::spawn_future_local(glib::clone!(
            #[weak(rename_to = tools)]
            self,
            async move {
                let reply = tools
                    .call_service(
                        PageTool::Lyrembed,
                        "FetchLyrics",
                        Some(args),
                        Some("(s)"),
                        10_000,
                    )
                    .await;
                match reply.map(|r| r.get::<(String,)>()) {
                    Ok(Some((run,))) => {
                        tracing::info!("lyrembed lyrics search {run} on {}", dir.display());
                        tools.imp().auto_run.replace(Some(run));
                    }
                    Ok(None) => tools.lyrics_job_ended(None),
                    Err(mut err) => {
                        gio::DBusError::strip_remote_error(&mut err);
                        tracing::info!("lyrembed's FetchLyrics: {err}");
                        if let Some(program) = program {
                            tools.fetch_lyrics_with(&program, &dir);
                        } else {
                            tools.message(err.message());
                            tools.lyrics_job_ended(None);
                        }
                    }
                }
            }
        ));
    }

    /// A lyrics search over D-Bus ended (`None`: it didn't start).
    fn lyrics_job_ended(&self, code: Option<i32>) {
        let imp = self.imp();
        imp.auto_run.replace(None);
        imp.auto_running.set(false);
        self.update_lyrics_jobs();
        if let Some(code) = code {
            self.lyrics_job_done(code);
        }
        self.run_next_lyrics_job();
    }

    fn lyrics_job_done(&self, code: i32) {
        let text = match lyrembed::auto_result(code) {
            // 0 may also mean nothing confident was found; the rescan shows.
            AutoResult::Done => gettext("Lyrics search finished"),
            AutoResult::SomeFailed => gettext("Some files couldn't be updated by lyrembed"),
            AutoResult::Unusable => gettext("lyrembed can't use this folder"),
            AutoResult::Cancelled => gettext("Lyrics search stopped"),
            AutoResult::Other(_) => gettext("lyrembed failed"),
        };
        self.message(&text);
        GatefoldApplication::get().library().rescan();
    }

    // cdport (§10.2) ---------------------------------------------------------

    fn cdport(&self) -> Cdport {
        self.imp().detected.borrow().cdport.clone()
    }

    /// The library folder to hand cdport (`None`: its own is in the
    /// player's folders).
    fn cdport_library(&self, config: &cdport::Config) -> Option<PathBuf> {
        let folders = library_folders::load(self.settings());
        cdport::library_arg(config, &folders).map(Path::to_path_buf)
    }

    /// "Import CD…"
    pub fn import_cd(&self) {
        self.start_import(None);
    }

    /// "Continue" an unfinished import (the first one).
    pub fn continue_import(&self) {
        let Some(dir) = self.imp().unfinished.borrow().first().cloned() else {
            return;
        };
        self.start_import(Some(dir));
    }

    fn start_import(&self, folder: Option<PathBuf>) {
        let cdport = self.cdport();
        if !cdport.usable() {
            self.missing_tool("cdport");
            return;
        }
        self.imp().import_started.set(Some(crate::metadata::now()));
        if cdport.dbus {
            self.open_page(PageStart::Service {
                tool: PageTool::Cdport,
                folder,
            });
            return;
        }
        let Some(program) = cdport.program.map(|p| p.path) else {
            return;
        };
        let library = self.cdport_library(&cdport.config);
        let argv = match folder {
            Some(dir) => cdport::continue_args(&program, &dir, library.as_deref()),
            None => cdport::import_args(&program, library.as_deref()),
        };
        self.open_page(PageStart::Command {
            tool: PageTool::Cdport,
            argv,
            status: PageTool::Cdport.status(),
        });
    }

    /// When the last import started, for the "New album" toast.
    pub fn import_started(&self) -> Option<i64> {
        self.imp().import_started.get()
    }

    pub fn forget_import(&self) {
        self.imp().import_started.set(None);
    }

    fn check_unfinished_imports(&self) {
        let cdport = self.cdport();
        if cdport.dbus {
            glib::spawn_future_local(glib::clone!(
                #[weak(rename_to = tools)]
                self,
                async move {
                    let reply = tools
                        .call_service(
                            PageTool::Cdport,
                            "UnfinishedAlbums",
                            None,
                            Some("(as)"),
                            10_000,
                        )
                        .await;
                    match reply.map(|r| r.get::<(Vec<String>,)>()) {
                        Ok(Some((folders,))) => {
                            tools.set_unfinished(folders.into_iter().map(PathBuf::from).collect());
                        }
                        Ok(None) => {}
                        Err(err) => tracing::info!("cdport's unfinished albums: {err}"),
                    }
                }
            ));
            return;
        }
        if cdport.program.is_none() {
            return;
        }
        let Some(work_dir) = cdport.config.work_dir else {
            return;
        };
        self.set_unfinished(cdport::unfinished_imports(&work_dir));
    }

    fn set_unfinished(&self, found: Vec<PathBuf>) {
        let count = found.len() as u32;
        self.imp().unfinished.replace(found);
        if self.imp().unfinished_imports.replace(count) != count {
            self.notify_unfinished_imports();
        }
    }

    // The tools over D-Bus -----------------------------------------------------

    /// Follows the tools' runs: their end (`Finished`), and a service going
    /// away while one of ours runs (it would have crashed).
    fn watch_services(&self) {
        glib::spawn_future_local(glib::clone!(
            #[weak(rename_to = tools)]
            self,
            async move {
                let bus = match gio::bus_get_future(gio::BusType::Session).await {
                    Ok(bus) => bus,
                    Err(err) => {
                        tracing::warn!("No session bus for the companion tools: {err}");
                        return;
                    }
                };
                for tool in [PageTool::Cdport, PageTool::Lyrembed] {
                    let finished = bus.subscribe_to_signal(
                        Some(tool.bus_name()),
                        Some(tool.interface()),
                        Some("Finished"),
                        Some(tool.object_path()),
                        None,
                        gio::DBusSignalFlags::NONE,
                        glib::clone!(
                            #[weak]
                            tools,
                            move |signal| {
                                // (run, exit code, …): cdport adds the album.
                                let parameters = signal.parameters;
                                if parameters.n_children() < 2 {
                                    return;
                                }
                                let run = parameters.child_value(0).get::<String>();
                                let code = parameters.child_value(1).get::<i32>();
                                if let (Some(run), Some(code)) = (run, code) {
                                    tools.service_run_finished(tool, &run, code);
                                }
                            }
                        ),
                    );
                    tools.imp().finished_signals.borrow_mut().push(finished);
                    gio::bus_watch_name_on_connection(
                        &bus,
                        tool.bus_name(),
                        gio::BusNameWatcherFlags::NONE,
                        |_, _, _| {},
                        glib::clone!(
                            #[weak]
                            tools,
                            move |_, _| tools.service_gone(tool)
                        ),
                    );
                }
                let _ = tools.imp().bus.set(bus);
            }
        ));
    }

    async fn call_service(
        &self,
        tool: PageTool,
        method: &str,
        args: Option<glib::Variant>,
        reply: Option<&str>,
        timeout_ms: i32,
    ) -> Result<glib::Variant, glib::Error> {
        let bus = match self.imp().bus.get() {
            Some(bus) => bus.clone(),
            None => gio::bus_get_future(gio::BusType::Session).await?,
        };
        bus.call_future(
            Some(tool.bus_name()),
            tool.object_path(),
            tool.interface(),
            method,
            args.as_ref(),
            reply.and_then(|r| glib::VariantTy::new(r).ok()),
            gio::DBusCallFlags::NONE,
            timeout_ms,
        )
        .await
    }

    /// The running page is this run of `tool`'s service.
    fn is_page_run(&self, tool: PageTool, run: &str) -> bool {
        let imp = self.imp();
        imp.page_tool.get() == Some(tool)
            && matches!(&*imp.page_run.borrow(), Some(PageRun::Service(id)) if id == run)
    }

    /// cdport's `Import` (the page's address comes back, Gatefold opens
    /// it) or lyrembed's `OpenPage` (it opens the page itself).
    fn start_service_run(&self, tool: PageTool, folder: Option<PathBuf>) {
        self.page_opened(tool, PageRun::Service(String::new()), None, tool.status());
        if tool == PageTool::Lyrembed {
            // lyrembed's editor plays the track in the page (§10.0).
            GatefoldApplication::get().player().pause();
        }
        let generation = self.imp().page_generation.get();
        let folder_text = folder
            .as_ref()
            .map(|f| f.to_string_lossy().into_owned())
            .unwrap_or_default();
        let (method, args, reply) = match tool {
            PageTool::Cdport => {
                let options = glib::VariantDict::new(None);
                if let Some(library) = self.cdport_library(&self.cdport().config) {
                    options.insert("Library", library.to_string_lossy().as_ref());
                }
                if folder.is_some() {
                    options.insert("Folder", folder_text.as_str());
                }
                (
                    "Import",
                    glib::Variant::tuple_from_iter([options.end()]),
                    "(ss)",
                )
            }
            PageTool::Lyrembed => (
                "OpenPage",
                glib::Variant::tuple_from_iter([folder_text.to_variant()]),
                "(sb)",
            ),
        };
        glib::spawn_future_local(glib::clone!(
            #[weak(rename_to = tools)]
            self,
            async move {
                let reply = tools
                    .call_service(tool, method, Some(args), Some(reply), START_TIMEOUT_MS)
                    .await;
                let imp = tools.imp();
                if imp.page_generation.get() != generation {
                    return;
                }
                let started = reply.map(|reply| {
                    let run = reply.child_value(0).get::<String>();
                    // cdport: the address; lyrembed: whether it opened.
                    let second = reply.child_value(1);
                    (run, second.get::<String>(), second.get::<bool>())
                });
                match started {
                    Ok((Some(run), url, opened)) => {
                        tracing::info!("{} run {run} started", tool.name());
                        imp.page_run.replace(Some(PageRun::Service(run.clone())));
                        imp.page_url.replace(url);
                        if imp.stop_when_started.take() {
                            tools.stop_service_run(tool, run);
                        } else if tool == PageTool::Cdport {
                            tools.show_page();
                        } else if opened == Some(false) {
                            tools.message(&gettext("lyrembed couldn't open your browser"));
                        }
                    }
                    Ok(_) => tools.page_closed(tool, -1),
                    Err(mut err) => {
                        gio::DBusError::strip_remote_error(&mut err);
                        tracing::warn!("{} couldn't start: {err}", tool.name());
                        tools.page_closed(tool, -1);
                        // Translators: {tool} is a program name, {message} its own message.
                        tools.message(
                            &gettext("{tool} couldn't start: {message}")
                                .replace("{tool}", tool.name())
                                .replace("{message}", err.message()),
                        );
                    }
                }
            }
        ));
    }

    fn stop_service_run(&self, tool: PageTool, run: String) {
        glib::spawn_future_local(glib::clone!(
            #[weak(rename_to = tools)]
            self,
            async move {
                let args = glib::Variant::tuple_from_iter([run.to_variant()]);
                if let Err(err) = tools
                    .call_service(tool, "Stop", Some(args), None, 10_000)
                    .await
                {
                    // Ended meanwhile (UnknownRun): nothing to stop.
                    tracing::info!("{}'s Stop: {err}", tool.name());
                    if tools.is_page_run(tool, &run) {
                        tools.page_closed(tool, -1);
                    }
                }
            }
        ));
    }

    fn service_run_finished(&self, tool: PageTool, run: &str, code: i32) {
        if self.is_page_run(tool, run) {
            tracing::info!("{} run {run} finished with {code}", tool.name());
            self.page_closed(tool, code);
        } else if tool == PageTool::Lyrembed && self.imp().auto_run.borrow().as_deref() == Some(run)
        {
            tracing::info!("lyrembed lyrics search {run} finished with {code}");
            self.lyrics_job_ended(Some(code));
        }
    }

    /// A service went away: its runs of ours ended with it.
    fn service_gone(&self, tool: PageTool) {
        let imp = self.imp();
        let page = imp.page_tool.get() == Some(tool)
            && matches!(&*imp.page_run.borrow(), Some(PageRun::Service(id)) if !id.is_empty());
        if page {
            tracing::warn!("{}'s D-Bus service went away during a run", tool.name());
            self.page_closed(tool, -1);
        }
        if tool == PageTool::Lyrembed && imp.auto_run.borrow().is_some() {
            tracing::warn!("lyrembed's D-Bus service went away during a lyrics search");
            self.lyrics_job_ended(Some(-1));
        }
    }

    /// A page run still going (Gatefold was restarted meanwhile): shown in
    /// the status bar again, so it can be shown and stopped.
    fn adopt_service_run(&self, tool: PageTool) {
        if self.page_running() {
            return;
        }
        glib::spawn_future_local(glib::clone!(
            #[weak(rename_to = tools)]
            self,
            async move {
                let (signature, kind_at) = match tool {
                    // (run, address)
                    PageTool::Cdport => ("(a(ss))", None),
                    // (run, kind, folder)
                    PageTool::Lyrembed => ("(a(sss))", Some(1)),
                };
                let Ok(reply) = tools
                    .call_service(tool, "Runs", None, Some(signature), 10_000)
                    .await
                else {
                    return;
                };
                let runs = reply.child_value(0);
                let found = (0..runs.n_children())
                    .map(|i| runs.child_value(i))
                    .find(|run| {
                        kind_at.is_none_or(|at| {
                            run.child_value(at).get::<String>().as_deref() == Some("page")
                        })
                    });
                let Some(found) = found else {
                    return;
                };
                let Some(run) = found.child_value(0).get::<String>() else {
                    return;
                };
                if tools.page_running() {
                    return;
                }
                // cdport hands out the page's address, lyrembed doesn't.
                let url = match tool {
                    PageTool::Cdport => found.child_value(1).get::<String>(),
                    PageTool::Lyrembed => None,
                };
                tracing::info!("{} run {run} is still going", tool.name());
                tools.page_opened(tool, PageRun::Service(run), url, tool.status());
            }
        ));
    }

    /// Preferences' "Set Up": `<tool> --install-dbus-service`, then a new
    /// detection.
    pub fn set_up_service(&self, tool: PageTool) {
        let (program, arg) = match tool {
            PageTool::Cdport => (self.cdport().program, cdport::SERVICE_ARG),
            PageTool::Lyrembed => (self.lyrembed().program, lyrembed::SERVICE_ARG),
        };
        let Some(program) = program.map(|p| p.path) else {
            return;
        };
        let argv = vec![program.into_os_string(), arg.into()];
        self.spawn(
            &argv,
            |_| {},
            glib::clone!(
                #[weak(rename_to = tools)]
                self,
                move |code| {
                    // Translators: {} is a program name (lyrembed, cdport).
                    if code == 0 {
                        tools.message(
                            &gettext("{}'s D-Bus service is set up").replace("{}", tool.name()),
                        );
                    } else {
                        tracing::warn!("{} --install-dbus-service exited with {code}", tool.name());
                        tools.message(
                            &gettext("{}'s D-Bus service couldn't be set up")
                                .replace("{}", tool.name()),
                        );
                    }
                    tools.detect();
                }
            ),
        );
    }

    /// Audio CDs via GIO (`x-content/audio-cdda`).
    fn watch_cds(&self) {
        let monitor = gio::VolumeMonitor::get();
        let update = glib::clone!(
            #[weak(rename_to = tools)]
            self,
            move || tools.update_cd_present()
        );
        let on_mount = update.clone();
        monitor.connect_mount_added(move |_, _| on_mount());
        let on_unmount = update.clone();
        monitor.connect_mount_removed(move |_, _| on_unmount());
        let _ = self.imp().volume_monitor.set(monitor);
        update();
    }

    fn update_cd_present(&self) {
        let Some(monitor) = self.imp().volume_monitor.get() else {
            return;
        };
        let mounts = monitor.mounts();
        glib::spawn_future_local(glib::clone!(
            #[weak(rename_to = tools)]
            self,
            async move {
                let mut present = false;
                for mount in mounts {
                    if let Ok(types) = mount.guess_content_type_future(false).await
                        && types.iter().any(|t| t == "x-content/audio-cdda")
                    {
                        present = true;
                    }
                }
                if tools.imp().cd_present.replace(present) != present {
                    tools.notify_cd_present();
                }
            }
        ));
    }

    // Tag editor (§10.3) -----------------------------------------------------

    pub fn editor(&self) -> Editor {
        let settings = self.settings();
        let mut name = settings.string("tag-editor").to_string();
        if name == "auto" {
            // Use cdport's choice, so both programs open the same editor.
            name = self
                .cdport()
                .config
                .editor
                .unwrap_or_else(|| "eartag".into());
        }
        Editor::from_name(&name, &settings.string("tag-editor-command"))
    }

    /// Opens files in the tag editor; saved changes arrive via inotify.
    pub fn edit_tags(&self, files: Vec<PathBuf>) {
        if files.is_empty() {
            return;
        }
        let editor = self.editor();
        let ear_tag_known = self.imp().detected.borrow().ear_tag;
        glib::spawn_future_local(glib::clone!(
            #[weak(rename_to = tools)]
            self,
            async move {
                // Ear Tag may have been installed meanwhile (from the
                // "Install" offer below): asked again before offering it.
                if editor == Editor::EarTag && !ear_tag_known {
                    let installed = gio::spawn_blocking(ear_tag_installed)
                        .await
                        .unwrap_or(false);
                    if !installed {
                        // "Install Ear Tag…": GNOME Software.
                        crate::spotify::open_in_browser(editor::EAR_TAG_INSTALL_URI);
                        return;
                    }
                    tools.imp().detected.borrow_mut().ear_tag = true;
                }
                let templates = editor.templates();
                let command = gio::spawn_blocking(move || {
                    templates
                        .iter()
                        .find_map(|t| runnable(t))
                        .map(|t| editor::expand(&t, &files))
                })
                .await
                .ok()
                .flatten();
                let Some(command) = command else {
                    tools.message(&gettext("{} isn't installed").replace("{}", editor.name()));
                    return;
                };
                let argv: Vec<OsString> = command.into_iter().map(OsString::from).collect();
                // Nothing to wait for, but an editor that fails right away
                // (e.g. Flatpak's sandbox setup) shouldn't do so silently.
                let started = std::time::Instant::now();
                let name = editor.name().to_owned();
                tools.spawn(
                    &argv,
                    |_| {},
                    glib::clone!(
                        #[weak]
                        tools,
                        move |code| {
                            if code == 0 {
                                return;
                            }
                            tracing::info!("Tag editor exited with {code}");
                            if started.elapsed() < std::time::Duration::from_secs(5) {
                                tools.message(
                                    &gettext("{} couldn't be opened").replace("{}", &name),
                                );
                            }
                        }
                    ),
                );
            }
        ));
    }

    /// The window offers the instructions (Preferences › Integrations).
    fn missing_tool(&self, name: &str) {
        self.emit_by_name::<()>("missing", &[&name]);
        // Looked for again, so it's found once installed (no restart).
        self.detect();
    }
}

impl Default for Tools {
    fn default() -> Self {
        Self::new()
    }
}

/// Runs on a background thread: find the tools and ask their versions.
fn detect(lyrembed_override: &Path, cdport_override: &Path) -> Detected {
    let probe = |name: &str, path: &Path| -> Option<ToolInfo> {
        let path = find_program(name, Some(path).filter(|p| !p.as_os_str().is_empty()))?;
        let version = run_for_output(&[path.as_os_str(), OsStr::new("--version")])
            .as_deref()
            .and_then(parse_version);
        Some(ToolInfo { path, version })
    };
    let lyrembed = detect_lyrembed(probe("lyrembed", lyrembed_override));
    // Without D-Bus, romanizing needs `romanize --stdio` (1.1.1).
    let romanize_stdio = !lyrembed.dbus
        && lyrembed.program.as_ref().is_some_and(|tool| {
            run_for_output(&[
                tool.path.as_os_str(),
                OsStr::new("romanize"),
                OsStr::new("--help"),
            ])
            .is_some_and(|help| lyrembed::supports_romanize_stdio(&help))
        });
    let cdport_program = cdport::PROGRAMS
        .iter()
        .find_map(|name| probe(name, cdport_override));
    Detected {
        cdport: detect_cdport(cdport_program),
        lyrembed,
        romanize_stdio,
        ear_tag: ear_tag_installed(),
    }
}

/// cdport over D-Bus (its `GetInfo`) and as a program; the settings from
/// D-Bus, else from its config file.
fn detect_cdport(program: Option<ToolInfo>) -> Cdport {
    let mut cdport = Cdport {
        version: program.as_ref().and_then(|p| p.version),
        program,
        ..Cdport::default()
    };
    match service_info(PageTool::Cdport) {
        Ok(info) => {
            cdport.dbus = true;
            cdport.version = info_text(&info, "Version")
                .as_deref()
                .and_then(parse_version)
                .or(cdport.version);
            cdport.config = cdport::Config {
                library_dir: info_text(&info, "LibraryDir").map(PathBuf::from),
                work_dir: info_text(&info, "WorkDir").map(PathBuf::from),
                editor: info_text(&info, "Editor"),
            };
        }
        Err(error) => {
            cdport.dbus_error = error;
            if cdport.program.is_some() {
                cdport.config = cdport::Config::read();
            }
        }
    }
    cdport
}

/// lyrembed over D-Bus (its `GetInfo`) and as a program; the settings from
/// D-Bus, else from its config file.
fn detect_lyrembed(program: Option<ToolInfo>) -> Lyrembed {
    let mut lyrembed = Lyrembed {
        version: program.as_ref().and_then(|p| p.version),
        program,
        ..Lyrembed::default()
    };
    match service_info(PageTool::Lyrembed) {
        Ok(info) => {
            lyrembed.dbus = true;
            lyrembed.version = info_text(&info, "Version")
                .as_deref()
                .and_then(parse_version)
                .or(lyrembed.version);
            lyrembed.config = lyrembed::Config {
                romanization_layout: info_text(&info, "RomanizationLayout"),
                max_file_count: info
                    .lookup::<u32>("MaxFileCount")
                    .ok()
                    .flatten()
                    .map(u64::from),
            };
        }
        Err(error) => {
            lyrembed.dbus_error = error;
            if lyrembed.program.is_some() {
                lyrembed.config = lyrembed::Config::read();
            }
        }
    }
    lyrembed
}

fn info_text(info: &glib::VariantDict, key: &str) -> Option<String> {
    info.lookup::<String>(key)
        .ok()
        .flatten()
        .filter(|s| !s.is_empty())
}

/// A tool's `GetInfo` over D-Bus. `Err(None)`: no service set up;
/// `Err(Some(why))`: set up, but it didn't answer.
fn service_info(tool: PageTool) -> Result<glib::VariantDict, Option<String>> {
    let reply = call_service_sync(tool, "GetInfo", None, "(a{sv})", 15_000)?;
    Ok(glib::VariantDict::new(Some(&reply.child_value(0))))
}

/// A call from a worker thread. `Err(None)`: the service isn't there (not
/// set up); `Err(Some(why))`: it failed.
fn call_service_sync(
    tool: PageTool,
    method: &str,
    args: Option<&glib::Variant>,
    reply: &str,
    timeout_ms: i32,
) -> Result<glib::Variant, Option<String>> {
    let bus = gio::bus_get_sync(gio::BusType::Session, gio::Cancellable::NONE)
        .map_err(|e| Some(e.to_string()))?;
    bus.call_sync(
        Some(tool.bus_name()),
        tool.object_path(),
        tool.interface(),
        method,
        args,
        glib::VariantTy::new(reply).ok(),
        gio::DBusCallFlags::NONE,
        timeout_ms,
        gio::Cancellable::NONE,
    )
    .map_err(|mut err| {
        let missing = gio::DBusError::remote_error(&err).is_some_and(|name| {
            name == "org.freedesktop.DBus.Error.ServiceUnknown"
                || name == "org.freedesktop.DBus.Error.NameHasNoOwner"
        });
        if missing {
            return None;
        }
        gio::DBusError::strip_remote_error(&mut err);
        tracing::warn!("{}'s D-Bus service: {err}", tool.name());
        Some(err.message().to_owned())
    })
}

/// lyrembed's alias groups over D-Bus, as `--list-aliases` prints them
/// (from a worker thread: the library scan). `None` without the service.
pub fn lyrembed_aliases_over_dbus() -> Option<String> {
    let reply =
        call_service_sync(PageTool::Lyrembed, "ArtistAliases", None, "(aas)", 20_000).ok()?;
    let (groups,) = reply.get::<(Vec<Vec<String>>,)>()?;
    Some(gatefold_core::library::artists::Aliases::groups_to_text(
        &groups,
    ))
}

/// lyrembed's `Romanize` (from a worker thread): `Ok(None)` when it left
/// the lyrics as they are.
pub fn romanize_over_dbus(lyrics: &str, hint: Option<&str>) -> Result<Option<String>, String> {
    let args = (lyrics, hint.unwrap_or("")).to_variant();
    let reply = call_service_sync(PageTool::Lyrembed, "Romanize", Some(&args), "(sss)", 30_000)
        .map_err(|e| e.unwrap_or_else(|| "lyrembed's D-Bus service is gone".to_owned()))?;
    let (romanized, _language, reason) = reply
        .get::<(String, String, String)>()
        .ok_or("an unexpected answer")?;
    if romanized.is_empty() {
        tracing::debug!("lyrembed left the lyrics as they are: {reason}");
        return Ok(None);
    }
    Ok(Some(romanized))
}

fn ear_tag_installed() -> bool {
    std::process::Command::new("flatpak")
        .args(["info", editor::EAR_TAG_ID])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

/// Output (stdout + stderr) of a quick command, with a time limit. The
/// output is read while it runs (a full pipe would stall it), and a
/// command that's stopped is reaped (no zombie).
fn run_for_output(argv: &[&OsStr]) -> Option<String> {
    use std::io::Read;
    let mut child = std::process::Command::new(argv[0])
        .args(&argv[1..])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .ok()?;
    let read = |pipe: Option<Box<dyn Read + Send>>| {
        std::thread::spawn(move || {
            let mut text = String::new();
            if let Some(mut pipe) = pipe {
                let mut bytes = Vec::new();
                let _ = pipe.read_to_end(&mut bytes);
                text = String::from_utf8_lossy(&bytes).into_owned();
            }
            text
        })
    };
    let stdout = read(
        child
            .stdout
            .take()
            .map(|p| Box::new(p) as Box<dyn Read + Send>),
    );
    let stderr = read(
        child
            .stderr
            .take()
            .map(|p| Box::new(p) as Box<dyn Read + Send>),
    );
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    while child.try_wait().ok()?.is_none() {
        if std::time::Instant::now() > deadline {
            let _ = child.kill();
            let _ = child.wait();
            return None;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    let mut text = stdout.join().unwrap_or_default();
    text.push_str(&stderr.join().unwrap_or_default());
    Some(text)
}

/// The command template, if its program exists (Flatpak apps via
/// `flatpak info`), with the program as found: from the GNOME menu,
/// `~/.local/bin` isn't on PATH, where starting it would look.
fn runnable(template: &[String]) -> Option<Vec<String>> {
    match Editor::flatpak_id(template) {
        Some(id) => std::process::Command::new("flatpak")
            .args(["info", id])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .is_ok_and(|s| s.success())
            .then(|| template.to_vec()),
        None => {
            let program = template.first()?;
            let found = find_program(program, None)
                .or_else(|| Path::new(program).is_file().then(|| PathBuf::from(program)))?;
            let mut command = template.to_vec();
            command[0] = found.to_string_lossy().into_owned();
            Some(command)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn program(version: Option<Version>) -> Option<ToolInfo> {
        Some(ToolInfo {
            path: PathBuf::from("/home/u/.local/bin/cdport"),
            version,
        })
    }

    #[test]
    fn cdport_states() {
        let none = Cdport::default();
        assert!(!Lyrembed::default().usable());
        let old_lyrembed = Lyrembed {
            program: program(Some(Version(1, 1, 1))),
            version: Some(Version(1, 1, 1)),
            ..Lyrembed::default()
        };
        assert!(old_lyrembed.usable() && old_lyrembed.is_old());
        assert!(
            !Lyrembed {
                program: program(None),
                ..Lyrembed::default()
            }
            .is_old(),
            "lyrembed has no old name to tell by"
        );
        assert!(!none.usable() && !none.is_old() && !none.can_set_up_service());

        let old = Cdport {
            program: program(Some(Version(1, 2, 1))),
            version: Some(Version(1, 2, 1)),
            ..Cdport::default()
        };
        assert!(old.usable() && old.is_old() && !old.can_set_up_service());

        let without_service = Cdport {
            program: program(Some(Version(1, 3, 0))),
            version: Some(Version(1, 3, 0)),
            ..Cdport::default()
        };
        assert!(
            Cdport {
                program: program(None),
                ..Cdport::default()
            }
            .can_set_up_service(),
            "a cdport whose version isn't known isn't taken for cdimport"
        );
        assert!(without_service.usable() && !without_service.is_old());
        assert!(without_service.can_set_up_service());

        // Seen only over D-Bus (from a Flatpak): no program, still usable.
        let dbus = Cdport {
            dbus: true,
            version: Some(Version(1, 3, 0)),
            ..Cdport::default()
        };
        assert!(dbus.usable() && !dbus.is_old() && !dbus.can_set_up_service());
    }
}
