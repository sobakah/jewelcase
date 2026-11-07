//! Companion tools (CONCEPT.md §10): lyrembed, cdimport and the tag editor.
//!
//! Tools run as `gio::Subprocess`es with their output read on the main
//! loop. A tool with a page (lyrembed's editor, cdimport) prints its local
//! address, which opens in the default browser; a status bar offers
//! "Show Page" and "Stop" (SIGINT, as Ctrl+C). Results arrive through the
//! files: the library's inotify watcher picks them up.

use std::cell::{Cell, OnceCell, RefCell};
use std::collections::VecDeque;
use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::Duration;

use adw::prelude::*;
use gatefold_tools::editor::{self, Editor};
use gatefold_tools::lyrembed::AutoResult;
use gatefold_tools::{Version, cdimport, find_program, lyrembed, parse_version};
use gettextrs::gettext;
use gtk::glib::subclass::Signal;
use gtk::subclass::prelude::*;
use gtk::{gio, glib};

use crate::application::GatefoldApplication;
use crate::config::APP_ID;
use crate::library_folders;

/// SIGINT: the tools stop as with Ctrl+C (cdimport finishes cyanrip's log).
const SIGINT: i32 = 2;

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
    pub cdimport: Option<ToolInfo>,
    pub romanize_stdio: bool,
    pub ear_tag: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PageTool {
    Lyrembed,
    Cdimport,
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
        /// Album folders with an unfinished cdimport run.
        #[property(get)]
        pub unfinished_imports: Cell<u32>,
        /// lyrembed --auto jobs running or waiting.
        #[property(get)]
        pub lyrics_jobs: Cell<u32>,

        pub detected: RefCell<Detected>,
        pub page_process: RefCell<Option<gio::Subprocess>>,
        pub page_tool: Cell<Option<PageTool>>,
        pub page_url: RefCell<Option<String>>,
        /// A page to open once the running one has closed.
        pub pending_page: RefCell<Option<(PageTool, Vec<OsString>, String)>>,
        pub auto_queue: RefCell<VecDeque<PathBuf>>,
        pub auto_running: Cell<bool>,
        pub unfinished: RefCell<Vec<PathBuf>>,
        /// When the last cdimport run started (Unix seconds), for the
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
        for key in ["lyrembed-path", "cdimport-path"] {
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
            PathBuf::from(self.settings().string("cdimport-path").as_str()),
        );
        glib::spawn_future_local(glib::clone!(
            #[weak(rename_to = tools)]
            self,
            async move {
                let detected = gio::spawn_blocking(move || detect(&overrides.0, &overrides.1))
                    .await
                    .unwrap_or_default();
                tracing::info!("Companion tools: {detected:?}");
                tools.imp().detected.replace(detected);
                tools.check_unfinished_imports();
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
    fn open_page(&self, tool: PageTool, argv: Vec<OsString>, status: String) {
        let imp = self.imp();
        if imp.page_process.borrow().is_some() {
            // One at a time: offer to close the running one first.
            let open = imp.page_tool.get();
            glib::spawn_future_local(glib::clone!(
                #[weak(rename_to = tools)]
                self,
                async move {
                    if tools.confirm_replace_page(open).await {
                        tools.imp().pending_page.replace(Some((tool, argv, status)));
                        tools.stop_page();
                    }
                }
            ));
            return;
        }
        let process = self.spawn(
            &argv,
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
        imp.page_process.replace(Some(process));
        imp.page_tool.set(Some(tool));
        imp.page_url.replace(None);
        imp.page_status.replace(status);
        self.notify_page_status();
        if tool == PageTool::Lyrembed {
            // lyrembed's editor plays the track in the page (§10.0).
            GatefoldApplication::get().player().pause();
        }
    }

    fn page_closed(&self, tool: PageTool, code: i32) {
        let imp = self.imp();
        imp.page_process.replace(None);
        imp.page_tool.set(None);
        imp.page_url.replace(None);
        imp.page_status.replace(String::new());
        self.notify_page_status();
        tracing::info!("{tool:?} page closed with {code}");
        let library = GatefoldApplication::get().library();
        library.rescan();
        if tool == PageTool::Cdimport {
            self.check_unfinished_imports();
        }
        if let Some((tool, argv, status)) = imp.pending_page.take() {
            self.open_page(tool, argv, status);
        }
    }

    async fn confirm_replace_page(&self, open: Option<PageTool>) -> bool {
        let name = match open {
            Some(PageTool::Cdimport) => "cdimport",
            _ => "lyrembed",
        };
        let dialog = adw::AlertDialog::builder()
            // Translators: {} is a program name (lyrembed, cdimport).
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
    /// cdimport can finish cyanrip's log and remember where it was.
    pub fn stop_page(&self) {
        let Some(process) = self.imp().page_process.borrow().clone() else {
            return;
        };
        process.send_signal(SIGINT);
        glib::timeout_add_seconds_local_once(
            10,
            glib::clone!(
                #[weak(rename_to = tools)]
                self,
                move || {
                    let still_running = tools
                        .imp()
                        .page_process
                        .borrow()
                        .as_ref()
                        .is_some_and(|p| p == &process);
                    if still_running {
                        tracing::info!("The tool page is still finishing");
                        tools.message(&gettext("The tool is still finishing; it stops by itself"));
                    }
                }
            ),
        );
    }

    /// A tool with a page runs (a CD import, lyrembed's page).
    pub fn page_running(&self) -> bool {
        self.imp().page_process.borrow().is_some()
    }

    // lyrembed (§10.1) -------------------------------------------------------

    /// "Check & Edit in lyrembed…": the album folder's page.
    pub fn lyrembed_page(&self, album_dir: &Path) {
        let Some(program) = self.lyrembed_path() else {
            self.missing_tool("lyrembed", lyrembed::INSTALL_COMMAND);
            return;
        };
        self.open_page(
            PageTool::Lyrembed,
            lyrembed::page_args(&program, album_dir),
            gettext("lyrembed is open in your browser"),
        );
    }

    /// "Find Lyrics Automatically": `lyrembed --auto`, one album folder per
    /// call, one job at a time.
    pub fn find_lyrics(&self, album_dir: &Path) {
        if self.lyrembed_path().is_none() {
            self.missing_tool("lyrembed", lyrembed::INSTALL_COMMAND);
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

    // cdimport (§10.2) -------------------------------------------------------

    fn cdimport_library(&self) -> Option<PathBuf> {
        let config = cdimport::Config::read();
        let folders = library_folders::load(self.settings());
        cdimport::library_arg(&config, &folders).map(Path::to_path_buf)
    }

    /// "Import CD…"
    pub fn import_cd(&self) {
        let Some(program) = self.cdimport_path() else {
            self.missing_tool("cdimport", cdimport::INSTALL_COMMAND);
            return;
        };
        let library = self.cdimport_library();
        self.imp().import_started.set(Some(crate::metadata::now()));
        self.open_page(
            PageTool::Cdimport,
            cdimport::import_args(&program, library.as_deref()),
            gettext("Importing a CD with cdimport"),
        );
    }

    /// "Continue" an unfinished import (the first one).
    pub fn continue_import(&self) {
        let Some(program) = self.cdimport_path() else {
            return;
        };
        let Some(dir) = self.imp().unfinished.borrow().first().cloned() else {
            return;
        };
        let library = self.cdimport_library();
        self.imp().import_started.set(Some(crate::metadata::now()));
        self.open_page(
            PageTool::Cdimport,
            cdimport::continue_args(&program, &dir, library.as_deref()),
            gettext("Importing a CD with cdimport"),
        );
    }

    fn cdimport_path(&self) -> Option<PathBuf> {
        self.imp()
            .detected
            .borrow()
            .cdimport
            .as_ref()
            .map(|t| t.path.clone())
    }

    /// When the last import started, for the "New album" toast.
    pub fn import_started(&self) -> Option<i64> {
        self.imp().import_started.get()
    }

    pub fn forget_import(&self) {
        self.imp().import_started.set(None);
    }

    fn check_unfinished_imports(&self) {
        if self.cdimport_path().is_none() {
            return;
        }
        let Some(work_dir) = cdimport::Config::read().work_dir else {
            return;
        };
        let found = cdimport::unfinished_imports(&work_dir);
        let count = found.len() as u32;
        self.imp().unfinished.replace(found);
        if self.imp().unfinished_imports.replace(count) != count {
            self.notify_unfinished_imports();
        }
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
            // Use cdimport's choice, so both programs open the same editor.
            name = cdimport::Config::read()
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
        if editor == Editor::EarTag && !self.imp().detected.borrow().ear_tag {
            // "Install Ear Tag…": GNOME Software.
            gtk::UriLauncher::new(editor::EAR_TAG_INSTALL_URI).launch(
                GatefoldApplication::get().active_window().as_ref(),
                gio::Cancellable::NONE,
                |_| {},
            );
            return;
        }
        glib::spawn_future_local(glib::clone!(
            #[weak(rename_to = tools)]
            self,
            async move {
                let templates = editor.templates();
                let command = gio::spawn_blocking(move || {
                    templates
                        .iter()
                        .find(|t| runnable(t))
                        .map(|t| editor::expand(t, &files))
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

    fn missing_tool(&self, name: &str, install: &str) {
        // Translators: {tool} is a program name, {command} how to install it.
        self.message(
            &gettext("{tool} isn't installed. Install it with “{command}”")
                .replace("{tool}", name)
                .replace("{command}", install),
        );
    }
}

impl Default for Tools {
    fn default() -> Self {
        Self::new()
    }
}

/// Runs on a background thread: find the tools and ask their versions.
fn detect(lyrembed_override: &Path, cdimport_override: &Path) -> Detected {
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
        cdimport: probe("cdimport", cdimport_override),
        lyrembed,
        romanize_stdio,
        ear_tag: std::process::Command::new("flatpak")
            .args(["info", editor::EAR_TAG_ID])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .is_ok_and(|s| s.success()),
    }
}

/// Output (stdout + stderr) of a quick command, with a time limit.
fn run_for_output(argv: &[&OsStr]) -> Option<String> {
    let mut child = std::process::Command::new(argv[0])
        .args(&argv[1..])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .ok()?;
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    while child.try_wait().ok()?.is_none() {
        if std::time::Instant::now() > deadline {
            let _ = child.kill();
            return None;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    let output = child.wait_with_output().ok()?;
    let mut text = String::from_utf8_lossy(&output.stdout).into_owned();
    text.push_str(&String::from_utf8_lossy(&output.stderr));
    Some(text)
}

/// Whether a command template's program exists (Flatpak apps via `flatpak info`).
fn runnable(template: &[String]) -> bool {
    match Editor::flatpak_id(template) {
        Some(id) => std::process::Command::new("flatpak")
            .args(["info", id])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .is_ok_and(|s| s.success()),
        None => template.first().is_some_and(|program| {
            find_program(program, None).is_some() || Path::new(program).is_file()
        }),
    }
}
