//! Companion tools (CONCEPT.md §10): lyrembed, cdport and the tag editor.
//!
//! Tools run as `gio::Subprocess`es with their output read on the main
//! loop. A tool with a page (lyrembed's editor, cdport) prints its local
//! address, which opens in the default browser; a status bar offers
//! "Show Page" and "Stop" (SIGINT, as Ctrl+C). Results arrive through the
//! files: the library's inotify watcher picks them up.
//!
//! cdport 1.3.0 and newer is reached over D-Bus when its service is set up
//! (`cdport --install-dbus-service`): its runs then belong to cdport, not
//! to Gatefold, and it's the only way from a Flatpak.

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
/// cdport's Import answers once its page is up, within 20 s.
const IMPORT_TIMEOUT_MS: i32 = 30_000;

/// A found tool: its program and version.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolInfo {
    pub path: PathBuf,
    pub version: Option<Version>,
}

/// What detection found (runs on a background thread).
#[derive(Debug, Clone, Default)]
pub struct Detected {
    pub lyrembed: Option<ToolInfo>,
    pub cdport: Cdport,
    pub romanize_stdio: bool,
    pub ear_tag: bool,
}

/// cdport as found: over D-Bus, as a program here, or both.
#[derive(Debug, Clone, Default)]
pub struct Cdport {
    /// The program (`cdport`, or `cdimport` before 1.3.0). A Flatpak
    /// doesn't see it.
    pub program: Option<ToolInfo>,
    /// Its D-Bus service answered.
    pub dbus: bool,
    /// Why its D-Bus service didn't start, when it's set up but broken
    /// (e.g. cdport was removed or moved since).
    pub dbus_error: Option<String>,
    pub version: Option<Version>,
    pub config: cdport::Config,
}

impl Cdport {
    /// Gatefold can import CDs with it.
    pub fn usable(&self) -> bool {
        self.dbus || self.program.is_some()
    }

    /// cdimport 1.2.x, from before cdport's new name and D-Bus interface.
    pub fn is_old(&self) -> bool {
        let Some(program) = self.program.as_ref().filter(|_| !self.dbus) else {
            return false;
        };
        match self.version {
            Some(version) => version < cdport::DBUS_VERSION,
            // Its version unknown: the name tells.
            None => program.path.file_name().is_some_and(|n| n == "cdimport"),
        }
    }

    /// cdport 1.3.0 or newer, without its D-Bus service set up.
    pub fn can_set_up_service(&self) -> bool {
        !self.dbus && self.program.is_some() && !self.is_old()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PageTool {
    Lyrembed,
    Cdport,
}

/// A tool with a page, while it runs.
#[derive(Debug, Clone)]
pub enum PageRun {
    /// Started by Gatefold: it ends with Gatefold.
    Process(gio::Subprocess),
    /// A cdport run over D-Bus (its ID; empty until Import answered): it
    /// runs on without Gatefold.
    Cdport(String),
}

/// A page to open (now, or once the running one has closed).
#[derive(Debug, Clone)]
pub enum PageStart {
    Command {
        tool: PageTool,
        argv: Vec<OsString>,
        status: String,
    },
    /// cdport over D-Bus; `folder` continues an unfinished import.
    Cdport { folder: Option<PathBuf> },
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
        /// Stop was asked for before cdport's Import answered.
        pub stop_when_started: Cell<bool>,
        /// A page to open once the running one has closed.
        pub pending_page: RefCell<Option<PageStart>>,
        /// The session bus (cdport's D-Bus interface).
        pub bus: OnceCell<gio::DBusConnection>,
        pub finished_signal: RefCell<Option<gio::SignalSubscription>>,
        pub auto_queue: RefCell<VecDeque<PathBuf>>,
        pub auto_running: Cell<bool>,
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
        self.watch_cdport();
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
                let dbus = detected.cdport.dbus;
                tools.imp().detected.replace(detected);
                tools.check_unfinished_imports();
                if dbus {
                    tools.adopt_cdport_run();
                }
                tools.emit_by_name::<()>("detected", &[]);
            }
        ));
    }

    /// The lyrembed program for `--list-aliases` (also used by scans).
    pub fn lyrembed_path(&self) -> Option<PathBuf> {
        self.imp()
            .detected
            .borrow()
            .lyrembed
            .as_ref()
            .map(|t| t.path.clone())
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
            PageStart::Cdport { folder } => self.start_cdport_run(folder),
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
        let name = match open {
            Some(PageTool::Cdport) => "cdport",
            _ => "lyrembed",
        };
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

    /// Reopens the page's address (if the tab was lost).
    pub fn show_page(&self) {
        let Some(url) = self.imp().page_url.borrow().clone() else {
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
            // Import hasn't answered yet: stopped once it has.
            PageRun::Cdport(id) if id.is_empty() => imp.stop_when_started.set(true),
            PageRun::Cdport(id) => self.stop_cdport_run(id),
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
    /// started; cdport's runs over D-Bus go on).
    pub fn page_needs_gatefold(&self) -> bool {
        matches!(*self.imp().page_run.borrow(), Some(PageRun::Process(_)))
    }

    // lyrembed (§10.1) -------------------------------------------------------

    /// "Check & Edit in lyrembed…": the album folder's page.
    pub fn lyrembed_page(&self, album_dir: &Path) {
        let Some(program) = self.lyrembed_path() else {
            self.missing_tool("lyrembed");
            return;
        };
        self.open_page(PageStart::Command {
            tool: PageTool::Lyrembed,
            argv: lyrembed::page_args(&program, album_dir),
            status: gettext("lyrembed is open in your browser"),
        });
    }

    /// "Find Lyrics Automatically": `lyrembed --auto`, one album folder per
    /// call, one job at a time.
    pub fn find_lyrics(&self, album_dir: &Path) {
        if self.lyrembed_path().is_none() {
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
        let Some(program) = self.lyrembed_path() else {
            return;
        };
        imp.auto_running.set(true);
        self.update_lyrics_jobs();
        let process = self.spawn(
            &lyrembed::auto_args(&program, &dir),
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
        }
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
            self.open_page(PageStart::Cdport { folder });
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
            status: gettext("Importing a CD with cdport"),
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
                        .call_cdport("UnfinishedAlbums", None, Some("(as)"), 10_000)
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

    // cdport over D-Bus ------------------------------------------------------

    /// Follows cdport's runs: their end (`Finished`), and the service
    /// going away while one of ours runs (it would have crashed).
    fn watch_cdport(&self) {
        glib::spawn_future_local(glib::clone!(
            #[weak(rename_to = tools)]
            self,
            async move {
                let bus = match gio::bus_get_future(gio::BusType::Session).await {
                    Ok(bus) => bus,
                    Err(err) => {
                        tracing::warn!("No session bus for cdport: {err}");
                        return;
                    }
                };
                let finished = bus.subscribe_to_signal(
                    Some(cdport::BUS_NAME),
                    Some(cdport::INTERFACE),
                    Some("Finished"),
                    Some(cdport::OBJECT_PATH),
                    None,
                    gio::DBusSignalFlags::NONE,
                    glib::clone!(
                        #[weak]
                        tools,
                        move |signal| {
                            if let Some((run, code, album)) =
                                signal.parameters.get::<(String, i32, String)>()
                            {
                                tools.cdport_run_finished(&run, code, &album);
                            }
                        }
                    ),
                );
                tools.imp().finished_signal.replace(Some(finished));
                gio::bus_watch_name_on_connection(
                    &bus,
                    cdport::BUS_NAME,
                    gio::BusNameWatcherFlags::NONE,
                    |_, _, _| {},
                    glib::clone!(
                        #[weak]
                        tools,
                        move |_, _| {
                            let ours = matches!(
                                &*tools.imp().page_run.borrow(),
                                Some(PageRun::Cdport(id)) if !id.is_empty()
                            );
                            if ours {
                                tracing::warn!("cdport's D-Bus service went away during a run");
                                tools.page_closed(PageTool::Cdport, -1);
                            }
                        }
                    ),
                );
                let _ = tools.imp().bus.set(bus);
            }
        ));
    }

    async fn call_cdport(
        &self,
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
            Some(cdport::BUS_NAME),
            cdport::OBJECT_PATH,
            cdport::INTERFACE,
            method,
            args.as_ref(),
            reply.and_then(|r| glib::VariantTy::new(r).ok()),
            gio::DBusCallFlags::NONE,
            timeout_ms,
        )
        .await
    }

    /// cdport's `Import`: a run on its own page, whose address comes back.
    fn start_cdport_run(&self, folder: Option<PathBuf>) {
        let status = gettext("Importing a CD with cdport");
        self.page_opened(
            PageTool::Cdport,
            PageRun::Cdport(String::new()),
            None,
            status,
        );
        let generation = self.imp().page_generation.get();
        let options = glib::VariantDict::new(None);
        if let Some(library) = self.cdport_library(&self.cdport().config) {
            options.insert("Library", library.to_string_lossy().as_ref());
        }
        if let Some(folder) = folder {
            options.insert("Folder", folder.to_string_lossy().as_ref());
        }
        let args = glib::Variant::tuple_from_iter([options.end()]);
        glib::spawn_future_local(glib::clone!(
            #[weak(rename_to = tools)]
            self,
            async move {
                let reply = tools
                    .call_cdport("Import", Some(args), Some("(ss)"), IMPORT_TIMEOUT_MS)
                    .await;
                let imp = tools.imp();
                if imp.page_generation.get() != generation {
                    return;
                }
                match reply.map(|r| r.get::<(String, String)>()) {
                    Ok(Some((run, url))) => {
                        tracing::info!("cdport run {run} at {url}");
                        imp.page_run.replace(Some(PageRun::Cdport(run.clone())));
                        imp.page_url.replace(Some(url));
                        if imp.stop_when_started.take() {
                            tools.stop_cdport_run(run);
                        } else {
                            tools.show_page();
                        }
                    }
                    Ok(None) => tools.page_closed(PageTool::Cdport, -1),
                    Err(mut err) => {
                        gio::DBusError::strip_remote_error(&mut err);
                        tracing::warn!("cdport couldn't start an import: {err}");
                        tools.page_closed(PageTool::Cdport, -1);
                        // Translators: {} is cdport's own message.
                        tools.message(
                            &gettext("cdport couldn't start: {}").replace("{}", err.message()),
                        );
                    }
                }
            }
        ));
    }

    fn stop_cdport_run(&self, run: String) {
        glib::spawn_future_local(glib::clone!(
            #[weak(rename_to = tools)]
            self,
            async move {
                let args = glib::Variant::tuple_from_iter([run.to_variant()]);
                if let Err(err) = tools.call_cdport("Stop", Some(args), None, 10_000).await {
                    // Ended meanwhile (UnknownRun): nothing to stop.
                    tracing::info!("cdport's Stop: {err}");
                    let ours = matches!(
                        &*tools.imp().page_run.borrow(),
                        Some(PageRun::Cdport(id)) if *id == run
                    );
                    if ours {
                        tools.page_closed(PageTool::Cdport, -1);
                    }
                }
            }
        ));
    }

    fn cdport_run_finished(&self, run: &str, code: i32, album: &str) {
        let ours = matches!(
            &*self.imp().page_run.borrow(),
            Some(PageRun::Cdport(id)) if id == run
        );
        if !ours {
            return;
        }
        tracing::info!("cdport run {run} finished with {code} ({album:?})");
        self.page_closed(PageTool::Cdport, code);
    }

    /// A cdport run still going (Gatefold was restarted meanwhile): shown
    /// in the status bar again, so it can be shown and stopped.
    fn adopt_cdport_run(&self) {
        if self.page_running() {
            return;
        }
        glib::spawn_future_local(glib::clone!(
            #[weak(rename_to = tools)]
            self,
            async move {
                let Ok(reply) = tools
                    .call_cdport("Runs", None, Some("(a(ss))"), 10_000)
                    .await
                else {
                    return;
                };
                let Some((runs,)) = reply.get::<(Vec<(String, String)>,)>() else {
                    return;
                };
                let Some((run, url)) = runs.into_iter().next() else {
                    return;
                };
                if tools.page_running() {
                    return;
                }
                tracing::info!("cdport run {run} is still going");
                tools.page_opened(
                    PageTool::Cdport,
                    PageRun::Cdport(run),
                    Some(url),
                    gettext("Importing a CD with cdport"),
                );
            }
        ));
    }

    /// Preferences' "Set Up": `cdport --install-dbus-service`, then a new
    /// detection.
    pub fn set_up_cdport_service(&self) {
        let Some(program) = self.cdport().program.map(|p| p.path) else {
            return;
        };
        let argv = vec![program.into_os_string(), cdport::SERVICE_ARG.into()];
        self.spawn(
            &argv,
            |_| {},
            glib::clone!(
                #[weak(rename_to = tools)]
                self,
                move |code| {
                    if code == 0 {
                        tools.message(&gettext("cdport's D-Bus service is set up"));
                    } else {
                        tracing::warn!("cdport --install-dbus-service exited with {code}");
                        tools.message(&gettext("cdport's D-Bus service couldn't be set up"));
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
    let lyrembed = probe("lyrembed", lyrembed_override);
    let romanize_stdio = lyrembed.as_ref().is_some_and(|tool| {
        run_for_output(&[
            tool.path.as_os_str(),
            OsStr::new("romanize"),
            OsStr::new("--help"),
        ])
        .is_some_and(|help| lyrembed::supports_romanize_stdio(&help))
    });
    Detected {
        cdport: detect_cdport(|name| probe(name, cdport_override)),
        lyrembed,
        romanize_stdio,
        ear_tag: ear_tag_installed(),
    }
}

/// cdport over D-Bus (its `GetInfo`) and as a program; the settings from
/// D-Bus, else from its config file.
fn detect_cdport(probe: impl Fn(&str) -> Option<ToolInfo>) -> Cdport {
    let program = cdport::PROGRAMS.iter().find_map(|name| probe(name));
    match cdport_info() {
        Ok((version, config)) => Cdport {
            version: version.or_else(|| program.as_ref().and_then(|p| p.version)),
            program,
            dbus: true,
            dbus_error: None,
            config,
        },
        Err(dbus_error) => Cdport {
            version: program.as_ref().and_then(|p| p.version),
            config: if program.is_some() {
                cdport::Config::read()
            } else {
                cdport::Config::default()
            },
            program,
            dbus: false,
            dbus_error,
        },
    }
}

/// `GetInfo` over D-Bus: the version and settings. `Err(None)`: no service
/// set up; `Err(Some(why))`: set up, but it didn't answer.
fn cdport_info() -> Result<(Option<Version>, cdport::Config), Option<String>> {
    let bus = gio::bus_get_sync(gio::BusType::Session, gio::Cancellable::NONE)
        .map_err(|e| Some(e.to_string()))?;
    let reply = bus.call_sync(
        Some(cdport::BUS_NAME),
        cdport::OBJECT_PATH,
        cdport::INTERFACE,
        "GetInfo",
        None,
        glib::VariantTy::new("(a{sv})").ok(),
        gio::DBusCallFlags::NONE,
        15_000,
        gio::Cancellable::NONE,
    );
    let reply = match reply {
        Ok(reply) => reply,
        Err(mut err) => {
            let missing = gio::DBusError::remote_error(&err).is_some_and(|name| {
                name == "org.freedesktop.DBus.Error.ServiceUnknown"
                    || name == "org.freedesktop.DBus.Error.NameHasNoOwner"
            });
            if missing {
                return Err(None);
            }
            gio::DBusError::strip_remote_error(&mut err);
            tracing::warn!("cdport's D-Bus service doesn't answer: {err}");
            return Err(Some(err.message().to_owned()));
        }
    };
    let info = glib::VariantDict::new(Some(&reply.child_value(0)));
    let text = |key: &str| {
        info.lookup::<String>(key)
            .ok()
            .flatten()
            .filter(|s| !s.is_empty())
    };
    let version = text("Version").as_deref().and_then(parse_version);
    let config = cdport::Config {
        library_dir: text("LibraryDir").map(PathBuf::from),
        work_dir: text("WorkDir").map(PathBuf::from),
        editor: text("Editor"),
    };
    Ok((version, config))
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
