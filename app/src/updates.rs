//! Updates (CONCEPT.md §13, phase 9): new versions of Gatefold, of the
//! Soloist build and of soloistd, checked when Gatefold starts and twice a
//! day, shown in a banner and installed only when asked.
//!
//! - **Gatefold** (the RPM from GitHub releases): the newest release
//!   above this version. While the repository is private, GitHub answers
//!   only logged-in requests, so it's asked through the `gh` command when
//!   it's installed and logged in, else directly. The RPM is downloaded,
//!   checked against the SHA-256 GitHub lists for it, and installed with
//!   `pkexec dnf install` (GNOME asks for the password); then Gatefold
//!   offers to restart. Off in development builds and in a build with
//!   `-Dupdate_check=false` (a Flatpak updates through its repository).
//! - **Soloist** (managed by soloistd): Spotify's build on its CDN has
//!   another ETag than the one soloistd installed (`release.json`).
//!   Updating runs `soloistd update` and restarts the service; Gatefold
//!   reconnects by itself. (soloistd also updates by itself every six
//!   hours.)
//! - **soloistd**: a newer GitHub release than `soloistd version`;
//!   installed like Gatefold when it came as an RPM, else linked.

use std::cell::RefCell;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

use adw::prelude::*;
use gettextrs::gettext;
use gtk::{gio, glib};
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::application::GatefoldApplication;
use crate::config;
use crate::window::GatefoldWindow;

const GATEFOLD_REPO: &str = "sobakah/gatefold";
const SOLOISTD_REPO: &str = "MopigamesYT/soloistd";
const SOLOIST_CDN: &str = "https://soloist-builds.spotifycdn.com/";
/// After the start (it doesn't slow it down) …
const FIRST_CHECK_S: u32 = 8;
/// … and again while Gatefold stays open.
const CHECK_EVERY_S: u32 = 12 * 3600;

/// Something that can be updated.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Update {
    Gatefold(Release),
    Soloistd(Release),
    /// A new Soloist build, with its date (the CDN's Last-Modified).
    Soloist {
        etag: String,
        date: String,
    },
}

impl Update {
    fn key(&self) -> String {
        match self {
            Update::Gatefold(r) => format!("gatefold:{}", r.tag),
            Update::Soloistd(r) => format!("soloistd:{}", r.tag),
            Update::Soloist { etag, .. } => format!("soloist:{etag}"),
        }
    }

    fn title(&self) -> String {
        match self {
            Update::Gatefold(r) => {
                gettext("Gatefold {} is available").replace("{}", &r.version_text())
            }
            Update::Soloistd(r) => {
                gettext("soloistd {} is available").replace("{}", &r.version_text())
            }
            Update::Soloist { .. } => gettext("A new Soloist build is available"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Release {
    tag: String,
    version: (u32, u32, u32),
    notes: String,
    page: String,
    rpm: Option<Asset>,
    /// Asked through `gh` (also downloaded through it).
    via_gh: bool,
    repo: &'static str,
}

impl Release {
    fn version_text(&self) -> String {
        let (a, b, c) = self.version;
        format!("{a}.{b}.{c}")
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Asset {
    name: String,
    url: String,
    sha256: Option<String>,
}

thread_local! {
    /// Updates found and not handled yet; the banner shows the first.
    static PENDING: RefCell<Vec<Update>> = const { RefCell::new(Vec::new()) };
    /// Put off with "Later" (this session).
    static LATER: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
    /// Being installed: not offered again meanwhile (a second download
    /// would write the file the first install is using).
    static INSTALLING: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
}

/// The first update to offer: not put off, not being installed.
fn first_offered() -> Option<Update> {
    PENDING.with_borrow(|pending| {
        LATER.with_borrow(|later| {
            INSTALLING.with_borrow(|installing| {
                pending
                    .iter()
                    .find(|u| !later.contains(&u.key()) && !installing.contains(&u.key()))
                    .cloned()
            })
        })
    })
}

/// Starts the checks for `window` (when it opens, then twice a day).
pub fn start(window: &GatefoldWindow) {
    let window = window.downgrade();
    glib::timeout_add_seconds_local_once(FIRST_CHECK_S, {
        let window = window.clone();
        move || check(window)
    });
    glib::timeout_add_seconds_local(CHECK_EVERY_S, move || {
        if window.upgrade().is_none() {
            return glib::ControlFlow::Break;
        }
        check(window.clone());
        glib::ControlFlow::Continue
    });
}

fn settings() -> gio::Settings {
    gio::Settings::new(config::APP_ID)
}

/// Whether this Gatefold looks for its own updates: the RPM build (not a
/// development build, not a Flatpak), installed by dnf.
fn checks_itself() -> bool {
    if std::env::var_os("GATEFOLD_UPDATE_TEST").is_some() {
        return true;
    }
    config::update_check()
        && !config::is_development()
        && std::env::current_exe().is_ok_and(|exe| exe == Path::new("/usr/bin/gatefold"))
        && rpm_version("gatefold").is_some()
}

fn check(window: glib::WeakRef<GatefoldWindow>) {
    if !settings().boolean("check-updates") {
        return;
    }
    let current = test_version().unwrap_or(config::VERSION).to_owned();
    glib::spawn_future_local(async move {
        let found = gio::spawn_blocking(move || {
            let mut found = Vec::new();
            if checks_itself() {
                match newest_release(gatefold_repo(), &current) {
                    Ok(Some(release)) => found.push(Update::Gatefold(release)),
                    Ok(None) => {}
                    Err(err) => tracing::info!("No update check for Gatefold: {err}"),
                }
            }
            if let Some(update) = soloist_build() {
                found.push(update);
            }
            if let Some(installed) = soloistd_version() {
                match newest_release(SOLOISTD_REPO, &installed) {
                    Ok(Some(mut release)) => {
                        // Installed from here only when it came as an RPM
                        // (else a second copy would land in /usr/bin).
                        if !soloistd_from_rpm() {
                            release.rpm = None;
                        }
                        found.push(Update::Soloistd(release));
                    }
                    Ok(None) => {}
                    Err(err) => tracing::info!("No update check for soloistd: {err}"),
                }
            }
            found
        })
        .await
        .unwrap_or_default();
        let Some(window) = window.upgrade() else {
            return;
        };
        let skipped = settings().strv("skipped-updates");
        PENDING.with_borrow_mut(|pending| {
            *pending = found
                .into_iter()
                .filter(|u| !skipped.iter().any(|s| s == u.key().as_str()))
                .collect();
        });
        refresh_banner(&window);
        // Development aid: `GATEFOLD_SCREENSHOT_UPDATE=1` opens the dialog.
        if std::env::var_os("GATEFOLD_SCREENSHOT_UPDATE").is_some() {
            show_dialog(&window);
        }
    });
}

/// `GATEFOLD_UPDATE_REPO=owner/name` asks another (public) repository
/// for Gatefold's releases (tests: the private one needs `gh`'s login).
fn gatefold_repo() -> &'static str {
    static REPO: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    REPO.get_or_init(|| {
        std::env::var("GATEFOLD_UPDATE_REPO")
            .ok()
            .filter(|r| r.split('/').count() == 2)
            .unwrap_or_else(|| GATEFOLD_REPO.to_owned())
    })
}

/// `GATEFOLD_UPDATE_TEST=0.1.0` pretends to be that version (tests).
fn test_version() -> Option<&'static str> {
    static VERSION: std::sync::OnceLock<Option<String>> = std::sync::OnceLock::new();
    VERSION
        .get_or_init(|| {
            std::env::var("GATEFOLD_UPDATE_TEST")
                .ok()
                .filter(|v| parse_version(v).is_some())
        })
        .as_deref()
}

fn refresh_banner(window: &GatefoldWindow) {
    let first = first_offered().map(|u| u.title());
    window.set_update_banner(first.as_deref());
}

/// The banner's button: the first update's dialog.
pub fn show_dialog(window: &GatefoldWindow) {
    let Some(update) = first_offered() else {
        window.set_update_banner(None);
        return;
    };
    let (body, install_label) = match &update {
        Update::Gatefold(release) | Update::Soloistd(release) => {
            let mut body = notes_text(&release.notes);
            if release.rpm.is_none() {
                body.push_str("\n\n");
                body.push_str(&gettext(
                    "It can't be installed from here; the release page has it.",
                ));
            }
            (
                body,
                release.rpm.as_ref().map(|_| gettext("_Install")),
            )
        }
        Update::Soloist { date, .. } => (
            gettext("Spotify published a new Soloist build ({}). soloistd installs it and restarts Soloist; a Spotify song that plays stops for a moment. soloistd also updates by itself within six hours.")
                .replace("{}", date),
            Some(gettext("_Update")),
        ),
    };
    let dialog = adw::AlertDialog::builder()
        .heading(update.title())
        .body(body)
        .build();
    dialog.add_response("later", &gettext("_Later"));
    dialog.add_response("skip", &gettext("_Skip This Version"));
    if let Update::Gatefold(release) | Update::Soloistd(release) = &update {
        dialog.add_response("page", &gettext("Release _Page"));
        let page = release.page.clone();
        dialog.connect_response(Some("page"), move |dialog, _| {
            gtk::UriLauncher::new(&page).launch(
                dialog.root().and_downcast_ref::<gtk::Window>(),
                gio::Cancellable::NONE,
                |_| {},
            );
        });
    }
    if let Some(label) = install_label {
        dialog.add_response("install", &label);
        dialog.set_response_appearance("install", adw::ResponseAppearance::Suggested);
        dialog.set_default_response(Some("install"));
    }
    dialog.set_close_response("later");
    let key = update.key();
    dialog.connect_response(
        None,
        glib::clone!(
            #[weak]
            window,
            move |_, response| {
                match response {
                    "later" | "page" => LATER.with_borrow_mut(|later| later.push(key.clone())),
                    "skip" => {
                        let settings = settings();
                        let mut skipped: Vec<String> = settings
                            .strv("skipped-updates")
                            .iter()
                            .map(ToString::to_string)
                            .collect();
                        skipped.push(key.clone());
                        // Only the last few matter.
                        let excess = skipped.len().saturating_sub(10);
                        skipped.drain(..excess);
                        let _ = settings.set_strv("skipped-updates", skipped);
                        forget(&key);
                    }
                    "install" => install(&window, update.clone()),
                    _ => {}
                }
                refresh_banner(&window);
            }
        ),
    );
    dialog.present(Some(window));
}

fn forget(key: &str) {
    PENDING.with_borrow_mut(|pending| pending.retain(|u| u.key() != key));
}

/// Release notes as plain text: Markdown marks and the install section
/// left out, at most about 1200 characters.
fn notes_text(notes: &str) -> String {
    let mut lines = Vec::new();
    for line in notes.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("## Install") || trimmed.starts_with("### Install") {
            break;
        }
        let text = trimmed
            .trim_start_matches('#')
            .trim_start()
            .replace("**", "")
            .replace('`', "");
        let text = match text.strip_prefix("- ") {
            Some(rest) => format!("• {rest}"),
            None => text,
        };
        lines.push(text);
    }
    let mut text = lines.join("\n").trim().to_owned();
    while text.contains("\n\n\n") {
        text = text.replace("\n\n\n", "\n\n");
    }
    if text.chars().count() > 1200 {
        text = text.chars().take(1200).collect::<String>() + "…";
    }
    text
}

fn install(window: &GatefoldWindow, update: Update) {
    window.show_toast(&match &update {
        Update::Soloist { .. } => gettext("Updating Soloist…"),
        Update::Gatefold(_) => gettext("Downloading the update…"),
        Update::Soloistd(_) => gettext("Downloading the update…"),
    });
    let key = update.key();
    let started = INSTALLING.with_borrow_mut(|installing| {
        if installing.contains(&key) {
            return false;
        }
        installing.push(key.clone());
        true
    });
    if !started {
        return;
    }
    glib::spawn_future_local(glib::clone!(
        #[weak]
        window,
        async move {
            let job = update.clone();
            let result = gio::spawn_blocking(move || match &job {
                Update::Gatefold(release) | Update::Soloistd(release) => {
                    let path = download(release)?;
                    let installed = install_rpm(&path);
                    let _ = std::fs::remove_file(&path);
                    installed
                }
                Update::Soloist { .. } => {
                    gatefold_soloist::soloistd::update()?;
                    gatefold_soloist::soloistd::control_service("restart")
                }
            })
            .await
            .unwrap_or_else(|_| Err("the update stopped unexpectedly".to_owned()));
            INSTALLING.with_borrow_mut(|installing| installing.retain(|k| *k != key));
            // A failed or cancelled one is offered again.
            refresh_banner(&window);
            match result {
                Ok(()) => {
                    forget(&key);
                    refresh_banner(&window);
                    match update {
                        Update::Gatefold(release) => ask_restart(&window, &release),
                        Update::Soloistd(release) => window.show_toast(
                            &gettext("soloistd {} is installed")
                                .replace("{}", &release.version_text()),
                        ),
                        Update::Soloist { .. } => {
                            window.show_toast(&gettext("Soloist is up to date"));
                        }
                    }
                }
                // The password dialog was cancelled: nothing happened.
                Err(err) if err == CANCELLED => {}
                Err(err) => {
                    tracing::warn!("Update failed: {err}");
                    let dialog = adw::AlertDialog::builder()
                        .heading(gettext("The Update Failed"))
                        .body(err)
                        .build();
                    dialog.add_response("close", &gettext("_Close"));
                    dialog.present(Some(&window));
                }
            }
        }
    ));
}

const CANCELLED: &str = "cancelled";

fn ask_restart(window: &GatefoldWindow, release: &Release) {
    let dialog = adw::AlertDialog::builder()
        .heading(gettext("Gatefold {} Is Installed").replace("{}", &release.version_text()))
        .body(gettext(
            "It’s used from the next start. Restarting now stops what plays; the queue is kept.",
        ))
        .build();
    dialog.add_response("later", &gettext("_Later"));
    dialog.add_response("restart", &gettext("_Restart Now"));
    dialog.set_response_appearance("restart", adw::ResponseAppearance::Suggested);
    dialog.set_default_response(Some("restart"));
    dialog.set_close_response("later");
    dialog.connect_response(Some("restart"), |_, _| restart());
    dialog.present(Some(window));
}

/// Quits, and starts again once this instance is gone (a second start
/// while it still runs would only bring its window up).
fn restart() {
    let pid = std::process::id();
    let started = Command::new("sh")
        .args([
            "-c",
            "while kill -0 \"$1\" 2>/dev/null; do sleep 0.2; done; exec gatefold",
            "sh",
            &pid.to_string(),
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
    let mut helper = match started {
        Ok(helper) => helper,
        Err(err) => {
            tracing::warn!("Cannot restart Gatefold: {err}");
            return;
        }
    };
    let app = GatefoldApplication::get();
    app.activate_action("quit", None);
    // Closing may keep the app (background playback) or be cancelled
    // (a running tool asks first): quit for good once the windows are
    // gone, else the restart is off.
    glib::timeout_add_seconds_local_once(2, move || {
        let app = GatefoldApplication::get();
        if app.windows().is_empty() {
            app.quit();
        } else {
            let _ = helper.kill();
            let _ = helper.wait();
        }
    });
}

// ---- Checking (worker threads) ----

fn agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .user_agent(format!(
            "Gatefold/{} ( https://github.com/sobakah/gatefold )",
            config::VERSION
        ))
        .timeout_global(Some(Duration::from_secs(30)))
        .build()
        .into()
}

fn gh() -> Option<PathBuf> {
    ["/usr/bin/gh", "/usr/local/bin/gh"]
        .iter()
        .map(PathBuf::from)
        .find(|p| p.is_file())
}

/// The releases of `repo` (GitHub's JSON), through `gh` when it works.
fn releases(repo: &str) -> Result<(Value, bool), String> {
    let path = format!("repos/{repo}/releases?per_page=10");
    if let Some(gh) = gh() {
        let output = Command::new(gh)
            .args(["api", &path])
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .output();
        if let Ok(output) = output
            && output.status.success()
            && let Ok(value) = serde_json::from_slice(&output.stdout)
        {
            return Ok((value, true));
        }
    }
    let body = agent()
        .get(format!("https://api.github.com/{path}"))
        .header("Accept", "application/vnd.github+json")
        .call()
        .map_err(|e| e.to_string())?
        .body_mut()
        .read_to_string()
        .map_err(|e| e.to_string())?;
    let value = serde_json::from_str(&body).map_err(|e| e.to_string())?;
    Ok((value, false))
}

/// The newest release of `repo` above `current` (drafts aside; pre-
/// releases count, Gatefold's are all pre-releases for now).
fn newest_release(repo: &'static str, current: &str) -> Result<Option<Release>, String> {
    let current = parse_version(current).ok_or("unknown version")?;
    let (value, via_gh) = releases(repo)?;
    let list = value.as_array().ok_or("unexpected answer")?;
    let newest = list
        .iter()
        .filter(|r| !r["draft"].as_bool().unwrap_or(false))
        .filter_map(|r| {
            let tag = r["tag_name"].as_str()?;
            Some((parse_version(tag)?, r))
        })
        .filter(|(version, _)| *version > current)
        .max_by_key(|(version, _)| *version);
    Ok(newest.map(|(version, r)| Release {
        tag: r["tag_name"].as_str().unwrap_or_default().to_owned(),
        version,
        notes: r["body"].as_str().unwrap_or_default().to_owned(),
        page: r["html_url"].as_str().unwrap_or_default().to_owned(),
        rpm: rpm_asset(r),
        via_gh,
        repo,
    }))
}

/// "v0.2.1", "0.2.1", "soloistd 0.2.0" → (0, 2, 1).
fn parse_version(text: &str) -> Option<(u32, u32, u32)> {
    let word = text.split_whitespace().find(|w| {
        w.trim_start_matches('v')
            .starts_with(|c: char| c.is_ascii_digit())
    })?;
    let mut parts = word
        .trim_start_matches('v')
        .split(|c: char| !c.is_ascii_digit())
        .map(|p| p.parse::<u32>().ok());
    Some((
        parts.next()??,
        parts.next()??,
        parts.next().flatten().unwrap_or(0),
    ))
}

/// The binary RPM for this machine and Fedora release, if the release
/// has one and Gatefold (or soloistd) came as an RPM.
fn rpm_asset(release: &Value) -> Option<Asset> {
    let arch = std::env::consts::ARCH;
    let dist = fedora_dist();
    let assets = release["assets"].as_array()?;
    let candidates: Vec<&Value> = assets
        .iter()
        .filter(|a| {
            a["name"]
                .as_str()
                .is_some_and(|n| n.ends_with(&format!(".{arch}.rpm")))
        })
        .collect();
    // This Fedora release's build first, else one built for another.
    let asset = candidates
        .iter()
        .find(|a| {
            dist.as_deref().is_some_and(|d| {
                a["name"]
                    .as_str()
                    .unwrap_or_default()
                    .contains(&format!(".{d}."))
            })
        })
        .or(candidates.first())?;
    Some(Asset {
        name: asset["name"].as_str()?.to_owned(),
        url: asset["browser_download_url"].as_str()?.to_owned(),
        sha256: asset["digest"]
            .as_str()
            .and_then(|d| d.strip_prefix("sha256:"))
            .map(str::to_owned),
    })
}

/// "fc45", from /etc/os-release.
fn fedora_dist() -> Option<String> {
    let text = std::fs::read_to_string("/etc/os-release").ok()?;
    let id = text.lines().find_map(|l| l.strip_prefix("VERSION_ID="))?;
    Some(format!("fc{}", id.trim_matches('"')))
}

fn rpm_version(package: &str) -> Option<String> {
    let output = Command::new("rpm")
        .args(["-q", "--qf", "%{VERSION}", package])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

/// soloistd is the one dnf installed (`/usr/bin/soloistd`).
fn soloistd_from_rpm() -> bool {
    gatefold_soloist::soloistd::binary().is_some_and(|b| b == Path::new("/usr/bin/soloistd"))
        && rpm_version("soloistd").is_some()
}

fn soloistd_version() -> Option<String> {
    let binary = gatefold_soloist::soloistd::binary()?;
    let output = Command::new(binary)
        .arg("version")
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    let text = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    parse_version(&text).map(|_| text)
}

/// A Soloist build on Spotify's CDN other than the one soloistd has.
fn soloist_build() -> Option<Update> {
    let state = glib::user_data_dir().join("soloistd/release.json");
    let state: Value = serde_json::from_slice(&std::fs::read(state).ok()?).ok()?;
    let archive = state["archive"].as_str()?;
    let installed = state["etag"].as_str()?;
    // Only Spotify's own file names.
    if !archive.starts_with("soloist_release_") || archive.contains('/') {
        return None;
    }
    let response = agent()
        .head(format!("{SOLOIST_CDN}{archive}"))
        .call()
        .map_err(|err| tracing::info!("No update check for Soloist: {err}"))
        .ok()?;
    let etag = response.headers().get("etag")?.to_str().ok()?.to_owned();
    if etag == installed {
        return None;
    }
    let date = response
        .headers()
        .get("last-modified")
        .and_then(|d| d.to_str().ok())
        .and_then(|d| glib::DateTime::from_iso8601(&http_date_to_iso(d)?, None).ok())
        .and_then(|d| d.to_local().ok())
        .and_then(|d| d.format("%e %B %Y").ok())
        .map(|d| d.trim().to_owned())
        .unwrap_or_default();
    Some(Update::Soloist { etag, date })
}

/// "Fri, 06 Nov 2026 18:32:51 GMT" → "2026-11-06T18:32:51Z".
fn http_date_to_iso(date: &str) -> Option<String> {
    let parts: Vec<&str> = date.split_whitespace().collect();
    let [_, day, month, year, time, _] = parts.as_slice() else {
        return None;
    };
    let month = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ]
    .iter()
    .position(|m| m == month)?
        + 1;
    Some(format!("{year}-{month:02}-{day:0>2}T{time}Z"))
}

// ---- Installing (worker threads) ----

/// Downloads the release's RPM and checks its SHA-256.
fn download(release: &Release) -> Result<PathBuf, String> {
    let asset = release.rpm.as_ref().ok_or("no package")?;
    if asset.name.contains('/') || !asset.name.ends_with(".rpm") {
        return Err("unexpected package name".into());
    }
    let dir = glib::user_cache_dir().join(config::APP_ID).join("updates");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let path = dir.join(&asset.name);
    let _ = std::fs::remove_file(&path);
    let via_gh = release.via_gh && gh().is_some();
    if via_gh {
        let status = Command::new(gh().expect("checked"))
            .args([
                "release",
                "download",
                &release.tag,
                "-R",
                release.repo,
                "-p",
            ])
            .arg(&asset.name)
            .arg("-D")
            .arg(&dir)
            .arg("--clobber")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map_err(|e| e.to_string())?;
        if !status.success() {
            return Err(gettext("The download failed"));
        }
    } else {
        let mut response = agent().get(&asset.url).call().map_err(|e| e.to_string())?;
        let mut bytes = Vec::new();
        response
            .body_mut()
            .with_config()
            .limit(500 * 1024 * 1024)
            .reader()
            .read_to_end(&mut bytes)
            .map_err(|e| e.to_string())?;
        std::fs::write(&path, bytes).map_err(|e| e.to_string())?;
    }
    let data = std::fs::read(&path).map_err(|e| e.to_string())?;
    let sum = hex(&Sha256::digest(&data));
    match &asset.sha256 {
        Some(expected) if expected.eq_ignore_ascii_case(&sum) => Ok(path),
        Some(_) => {
            let _ = std::fs::remove_file(&path);
            Err(gettext(
                "The downloaded package isn’t the one GitHub lists (its checksum differs); it wasn’t installed.",
            ))
        }
        None => {
            let _ = std::fs::remove_file(&path);
            Err(gettext(
                "GitHub lists no checksum for this package, so it wasn’t installed. The release page has it.",
            ))
        }
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// `pkexec dnf install` (GNOME asks for the password).
fn install_rpm(path: &Path) -> Result<(), String> {
    let output = Command::new("pkexec")
        .args(["dnf", "install", "-y"])
        .arg(path)
        .stdin(Stdio::null())
        .output()
        .map_err(|e| e.to_string())?;
    match output.status.code() {
        Some(0) => Ok(()),
        // Not authorized or the dialog dismissed.
        Some(126 | 127) => Err(CANCELLED.to_owned()),
        _ => {
            let stderr = String::from_utf8_lossy(&output.stderr);
            Err(stderr
                .lines()
                .rev()
                .find(|l| !l.trim().is_empty())
                .unwrap_or("dnf failed")
                .trim()
                .to_owned())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_from_tags_and_version_lines() {
        assert_eq!(parse_version("v0.2.1"), Some((0, 2, 1)));
        assert_eq!(parse_version("0.3"), Some((0, 3, 0)));
        assert_eq!(parse_version("soloistd 0.2.0"), Some((0, 2, 0)));
        assert_eq!(parse_version("0.2.1-dev"), Some((0, 2, 1)));
        assert_eq!(parse_version("nightly"), None);
        assert!(parse_version("v0.10.0") > parse_version("v0.9.9"));
    }

    #[test]
    fn release_notes_without_markdown_and_install() {
        let notes = "Some intro.\n\n- **Bold** item with `code`\n\n### Install\n\nsudo dnf …";
        assert_eq!(notes_text(notes), "Some intro.\n\n• Bold item with code");
    }

    #[test]
    fn http_dates() {
        assert_eq!(
            http_date_to_iso("Fri, 06 Nov 2026 18:32:51 GMT").as_deref(),
            Some("2026-11-06T18:32:51Z")
        );
        assert_eq!(http_date_to_iso("garbage"), None);
    }

    #[test]
    fn the_rpm_for_this_machine() {
        let arch = std::env::consts::ARCH;
        let release = serde_json::json!({
            "assets": [
                {"name": "gatefold-0.2.1-1.fc45.src.rpm", "browser_download_url": "s", "digest": "sha256:aa"},
                {"name": format!("gatefold-0.2.1-1.fc45.{arch}.rpm"), "browser_download_url": "b", "digest": "sha256:bb"},
            ]
        });
        let asset = rpm_asset(&release).unwrap();
        assert_eq!(asset.url, "b");
        assert_eq!(asset.sha256.as_deref(), Some("bb"));
    }
}
