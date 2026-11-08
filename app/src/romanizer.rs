//! Automatic romanization for display (CONCEPT.md §5.1) with lyrembed's
//! engine: `lyrembed romanize --stdio`, one process started on first need
//! and stopped after 5 minutes without requests (Python and its
//! dictionaries take about a second to start). Results are kept in the
//! database by the lyrics text, so each text is romanized once. Nothing is
//! written to the files.

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use gatefold_lyrics::Lyrics;
use gtk::gio;
use gtk::glib;
use serde_json::{Value, json};

use crate::application::GatefoldApplication;
use crate::metadata::now;

/// The helper stops after this long without requests.
const IDLE: Duration = Duration::from_secs(5 * 60);

struct Helper {
    program: PathBuf,
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    last_used: Instant,
}

impl Drop for Helper {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

static HELPER: Mutex<Option<Helper>> = Mutex::new(None);

/// Whether the installed lyrembed can do it (Preferences → Integrations
/// says so too).
pub fn available() -> bool {
    GatefoldApplication::get().tools().detected().romanize_stdio
}

/// These lyrics with romanization under their non-Latin lines, from the
/// cache or lyrembed; `None` when there's nothing to add, or lyrembed
/// can't be asked (not installed, too old, failed).
pub async fn romanize(lyrics: &Lyrics) -> Option<Lyrics> {
    if !lyrics.needs_romanization() || !available() {
        return None;
    }
    let program = GatefoldApplication::get().tools().lyrembed_path()?;
    let text = lyrics.to_text();
    let hint = lyrics.han_hint();
    let key = gatefold_core::library::covers::cover_key(
        'r',
        format!("{}\n{text}", hint.unwrap_or("")).as_bytes(),
    );
    let library = GatefoldApplication::get().library();
    if let Some(cached) = library
        .db()
        .and_then(|db| db.romanized(&key).ok().flatten())
    {
        return cached.map(|text| lyrics.with_romanization(&text));
    }
    let request = text.clone();
    let answer = gio::spawn_blocking(move || ask(&program, &request, hint))
        .await
        .unwrap_or_else(|_| Err("the romanization job failed".into()));
    match answer {
        Ok(romanized) => {
            if let Some(mut db) = library.db()
                && let Err(err) = db.set_romanized(&key, romanized.as_deref(), now())
            {
                tracing::warn!("Cannot keep the romanization: {err}");
            }
            romanized.map(|text| lyrics.with_romanization(&text))
        }
        Err(err) => {
            tracing::warn!("lyrembed couldn't romanize: {err}");
            None
        }
    }
}

/// One request to the helper (started if needed). `Ok(None)`: nothing to
/// romanize (Latin, already romanized, a backend missing).
fn ask(program: &Path, lyrics: &str, hint: Option<&str>) -> Result<Option<String>, String> {
    let mut guard = HELPER.lock().unwrap_or_else(|e| e.into_inner());
    if guard.as_ref().is_some_and(|h| h.program != program) {
        *guard = None;
    }
    if guard.is_none() {
        *guard = Some(start(program)?);
    }
    let helper = guard.as_mut().expect("started above");
    helper.last_used = Instant::now();
    let request = json!({"id": 1, "lyrics": lyrics, "han_hint": hint});
    let mut answer = String::new();
    let result = writeln!(helper.stdin, "{request}")
        .and_then(|()| helper.stdin.flush())
        .and_then(|()| helper.stdout.read_line(&mut answer));
    match result {
        Ok(read) if read > 0 => {}
        // Gone (crashed, killed): started again next time.
        Ok(_) => {
            *guard = None;
            return Err("lyrembed stopped".into());
        }
        Err(err) => {
            *guard = None;
            return Err(err.to_string());
        }
    }
    let value: Value = serde_json::from_str(&answer).map_err(|e| e.to_string())?;
    if let Some(error) = value["error"].as_str() {
        return Err(error.to_owned());
    }
    match value["lyrics"].as_str() {
        Some(text) => Ok(Some(text.to_owned())),
        None => {
            tracing::debug!(
                "lyrembed left the lyrics as they are: {}",
                value["reason"].as_str().unwrap_or_default()
            );
            Ok(None)
        }
    }
}

fn start(program: &Path) -> Result<Helper, String> {
    let mut child = Command::new(program)
        .args(["romanize", "--stdio"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("cannot start lyrembed: {e}"))?;
    let stdin = child.stdin.take().ok_or("no stdin")?;
    let stdout = BufReader::new(child.stdout.take().ok_or("no stdout")?);
    tracing::debug!("Started lyrembed romanize --stdio");
    Ok(Helper {
        program: program.to_owned(),
        child,
        stdin,
        stdout,
        last_used: Instant::now(),
    })
}

/// Stops the helper after a while without requests (checked every
/// minute), and when Gatefold quits.
pub fn watch_idle() {
    glib::timeout_add_seconds_local(60, || {
        stop_if(|helper| helper.last_used.elapsed() >= IDLE);
        glib::ControlFlow::Continue
    });
}

pub fn stop() {
    stop_if(|_| true);
}

fn stop_if(condition: impl Fn(&Helper) -> bool) {
    // Not while a request runs (that one holds the lock): next minute.
    let Ok(mut guard) = HELPER.try_lock() else {
        return;
    };
    if guard.as_ref().is_some_and(condition) {
        tracing::debug!("Stopping lyrembed romanize --stdio");
        *guard = None;
    }
}
