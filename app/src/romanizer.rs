//! Automatic romanization for display (CONCEPT.md §5.1) with lyrembed's
//! engine: its D-Bus service's `Romanize` when it's set up (lyrembed 1.2.0),
//! else `lyrembed romanize --stdio`, one process started on first need and
//! stopped after 5 minutes without requests (Python and its dictionaries
//! take about a second to start). Results are kept in the
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
/// An answer takes this long at most (a long song in Japanese, the first
/// request with Python starting: a few seconds).
const ANSWER_TIMEOUT: Duration = Duration::from_secs(30);

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
    GatefoldApplication::get().tools().detected().can_romanize()
}

/// These lyrics with romanization under their non-Latin lines, from the
/// cache or lyrembed; `None` when there's nothing to add, or lyrembed
/// can't be asked (not installed, too old, failed).
pub async fn romanize(lyrics: &Lyrics) -> Option<Lyrics> {
    if !lyrics.needs_romanization() || !available() {
        return None;
    }
    let tools = GatefoldApplication::get().tools();
    // Over D-Bus (`None`), or the program.
    let program = if tools.detected().lyrembed.dbus {
        None
    } else {
        Some(tools.lyrembed_path()?)
    };
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
    let answer = gio::spawn_blocking(move || match program {
        Some(program) => ask(&program, &request, hint, ANSWER_TIMEOUT),
        None => crate::tools::romanize_over_dbus(&request, hint),
    })
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
fn ask(
    program: &Path,
    lyrics: &str,
    hint: Option<&str>,
    timeout: Duration,
) -> Result<Option<String>, String> {
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
    // A helper that doesn't answer is stopped, so reading ends and the lock
    // is free again (otherwise every later request would wait for good).
    let answered = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let watchdog = {
        let answered = answered.clone();
        let pid = libc::pid_t::try_from(helper.child.id()).ok();
        std::thread::spawn(move || {
            let start = Instant::now();
            while start.elapsed() < timeout {
                if answered.load(std::sync::atomic::Ordering::Relaxed) {
                    return;
                }
                std::thread::sleep(Duration::from_millis(100));
            }
            if let Some(pid) = pid
                && !answered.load(std::sync::atomic::Ordering::Relaxed)
            {
                tracing::warn!("lyrembed doesn't answer; stopping it");
                // SAFETY: a plain signal to our own child, still running
                // (it's only reaped after the answer or this signal).
                unsafe {
                    libc::kill(pid, libc::SIGKILL);
                }
            }
        })
    };
    let result = writeln!(helper.stdin, "{request}")
        .and_then(|()| helper.stdin.flush())
        .and_then(|()| helper.stdout.read_line(&mut answer));
    answered.store(true, std::sync::atomic::Ordering::Relaxed);
    let _ = watchdog.join();
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

#[cfg(test)]
mod tests {
    use super::*;

    fn fake_lyrembed(name: &str, script: &str) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let path =
            std::env::temp_dir().join(format!("gatefold-test-{}-{name}", std::process::id()));
        std::fs::write(&path, format!("#!/bin/sh\n{script}\n")).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        path
    }

    #[test]
    fn a_helper_that_doesnt_answer_is_stopped_and_started_again() {
        let silent = fake_lyrembed("silent", "exec sleep 600");
        let start = Instant::now();
        let result = ask(&silent, "사랑해", None, Duration::from_millis(500));
        assert!(result.is_err());
        assert!(
            start.elapsed() < Duration::from_secs(5),
            "didn't wait for good"
        );
        assert!(
            HELPER.lock().unwrap().is_none(),
            "the stuck helper is gone, the next request starts a new one"
        );
        // One that answers works afterwards (the lock is free).
        let answering = fake_lyrembed(
            "answering",
            r#"while read line; do echo '{"id":1,"lyrics":"x\ny","language":"ko","reason":""}'; done"#,
        );
        let result = ask(&answering, "사랑해", None, Duration::from_secs(5));
        assert_eq!(result, Ok(Some("x\ny".to_owned())));
        stop();
        let _ = std::fs::remove_file(silent);
        let _ = std::fs::remove_file(answering);
    }
}
