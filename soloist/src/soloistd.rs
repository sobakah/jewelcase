//! soloistd (github.com/MopigamesYT/soloistd): a small helper that installs
//! and updates Soloist and keeps it running as a systemd user service.
//! Jewelcase can read and change its config.json, control the service and
//! start an update.
//!
//! Everything here blocks briefly (it runs commands); call it from a worker
//! thread.

use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde_json::{Map, Value};

/// The systemd user unit soloistd installs.
pub const SERVICE: &str = "soloistd.service";
/// The WebSocket address Jewelcase suggests (local only).
pub const DEFAULT_WEBSOCKET: &str = "127.0.0.1:9090";
pub const PROJECT_URL: &str = "https://github.com/MopigamesYT/soloistd";

/// soloistd's config.json (`JEWELCASE_SOLOISTD_CONFIG` for tests, which
/// must never reach the user's own).
pub fn config_path() -> PathBuf {
    if let Some(path) = std::env::var_os("JEWELCASE_SOLOISTD_CONFIG") {
        return PathBuf::from(path);
    }
    host_xdg("XDG_CONFIG_HOME", ".config").join("soloistd/config.json")
}

/// The host's XDG folder: in a Flatpak, `XDG_*` points into the sandbox's
/// own folders (Flatpak hands the host's over as `HOST_XDG_*` when set).
fn host_xdg(var: &str, fallback: &str) -> PathBuf {
    if std::path::Path::new("/.flatpak-info").exists() {
        return std::env::var_os(format!("HOST_{var}"))
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(fallback)))
            .unwrap_or_default();
    }
    xdg(var, fallback)
}

fn xdg(var: &str, fallback: &str) -> PathBuf {
    std::env::var_os(var)
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(fallback)))
        .unwrap_or_default()
}

/// The `soloistd` program, if installed.
pub fn binary() -> Option<PathBuf> {
    let mut candidates: Vec<PathBuf> = std::env::var_os("PATH")
        .map(|path| {
            std::env::split_paths(&path)
                .map(|d| d.join("soloistd"))
                .collect()
        })
        .unwrap_or_default();
    candidates.push(xdg("HOME", "").join(".local/bin/soloistd"));
    candidates.into_iter().find(|p| p.is_file())
}

/// The Soloist binary soloistd downloads and keeps up to date (the
/// Flatpak sees it read-only).
pub fn soloist_binary() -> PathBuf {
    host_xdg("XDG_DATA_HOME", ".local/share").join("soloistd/bin/soloist")
}

/// The Soloist binary soloistd manages, and its version.
pub fn soloist_version() -> Option<String> {
    let binary = soloist_binary();
    binary.is_file().then(|| crate::version(&binary)).flatten()
}

/// soloistd's settings, keeping keys Jewelcase doesn't know.
#[derive(Debug, Clone, PartialEq)]
pub struct Config {
    path: PathBuf,
    values: Map<String, Value>,
}

impl Config {
    pub fn load() -> Option<Self> {
        Self::load_from(&config_path())
    }

    pub fn load_from(path: &Path) -> Option<Self> {
        let text = std::fs::read_to_string(path).ok()?;
        let Value::Object(values) = serde_json::from_str(&text).ok()? else {
            return None;
        };
        Some(Self {
            path: path.to_path_buf(),
            values,
        })
    }

    pub fn string(&self, key: &str) -> Option<String> {
        self.values
            .get(key)
            .and_then(Value::as_str)
            .map(str::to_owned)
            .filter(|s| !s.is_empty())
    }

    pub fn number(&self, key: &str) -> Option<u64> {
        self.values.get(key).and_then(Value::as_u64)
    }

    /// `None` or an empty value removes the setting (soloistd's default).
    pub fn set_string(&mut self, key: &str, value: Option<&str>) {
        match value.map(str::trim).filter(|v| !v.is_empty()) {
            Some(value) => {
                self.values.insert(key.to_owned(), Value::from(value));
            }
            None => {
                self.values.remove(key);
            }
        }
    }

    pub fn set_number(&mut self, key: &str, value: Option<u64>) {
        match value {
            Some(value) => {
                self.values.insert(key.to_owned(), Value::from(value));
            }
            None => {
                self.values.remove(key);
            }
        }
    }

    pub fn device_name(&self) -> Option<String> {
        self.string("device_name")
    }

    pub fn websocket(&self) -> Option<String> {
        self.string("websocket")
    }

    pub fn has_api_key(&self) -> bool {
        self.string("api_key").is_some()
    }

    /// Writes the file as soloistd does: only readable by the user (it
    /// holds the API key), replaced in one step.
    pub fn save(&self) -> std::io::Result<()> {
        if let Some(dir) = self.path.parent() {
            std::fs::create_dir_all(dir)?;
            let _ = std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700));
        }
        let text = serde_json::to_string_pretty(&Value::Object(self.values.clone()))
            .map_err(std::io::Error::other)?;
        let temporary = self.path.with_extension("json.jewelcase");
        {
            let mut file = std::fs::OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(true)
                .open(&temporary)?;
            file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
            file.write_all(text.as_bytes())?;
            file.write_all(b"\n")?;
            file.sync_all()?;
        }
        std::fs::rename(&temporary, &self.path)
    }
}

/// The systemd user service.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServiceState {
    Running,
    Starting,
    Stopped,
    Failed,
    /// soloistd isn't installed as a service (`soloistd service install`).
    NotInstalled,
    /// systemd can't be asked (no user session).
    Unknown,
}

pub fn service_state() -> ServiceState {
    let Ok(output) = Command::new("systemctl")
        .args([
            "--user",
            "show",
            SERVICE,
            "--property=LoadState,ActiveState",
        ])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
    else {
        return ServiceState::Unknown;
    };
    if !output.status.success() {
        return ServiceState::Unknown;
    }
    parse_service_state(&String::from_utf8_lossy(&output.stdout))
}

fn parse_service_state(text: &str) -> ServiceState {
    let field = |name: &str| {
        text.lines()
            .find_map(|l| l.strip_prefix(name)?.strip_prefix('='))
            .unwrap_or("")
            .trim()
            .to_owned()
    };
    if field("LoadState") == "not-found" {
        return ServiceState::NotInstalled;
    }
    match field("ActiveState").as_str() {
        "active" | "reloading" => ServiceState::Running,
        "activating" | "deactivating" => ServiceState::Starting,
        "failed" => ServiceState::Failed,
        "inactive" => ServiceState::Stopped,
        _ => ServiceState::Unknown,
    }
}

/// `start`, `stop` or `restart` the service.
pub fn control_service(action: &str) -> Result<(), String> {
    run("systemctl", &["--user", action, SERVICE])
}

/// The service's latest log lines.
pub fn logs(lines: u32) -> String {
    Command::new("journalctl")
        .args([
            "--user",
            "--unit",
            SERVICE,
            "--lines",
            &lines.to_string(),
            "--no-pager",
            "--output",
            "short",
        ])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
        .unwrap_or_default()
}

/// Downloads the current Soloist build now (`soloistd update`).
pub fn update() -> Result<(), String> {
    let binary = binary().ok_or("soloistd isn't installed")?;
    run(&binary.to_string_lossy(), &["update"])
}

fn run(program: &str, args: &[&str]) -> Result<(), String> {
    let output = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .output()
        .map_err(|e| e.to_string())?;
    if output.status.success() {
        Ok(())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        Err(stderr
            .lines()
            .rev()
            .find(|l| !l.trim().is_empty())
            .unwrap_or("failed")
            .trim()
            .to_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_edits_keep_unknown_keys_and_stay_private() {
        let dir = std::env::temp_dir().join(format!("jewelcase-soloistd-{}", std::process::id()));
        let path = dir.join("config.json");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            &path,
            r#"{"api_key":"secret","device_name":"PC","cache_size_mb":200,"future_key":true}"#,
        )
        .unwrap();
        let mut config = Config::load_from(&path).unwrap();
        assert_eq!(config.device_name().as_deref(), Some("PC"));
        assert!(config.has_api_key());
        assert_eq!(config.websocket(), None);
        config.set_string("websocket", Some(DEFAULT_WEBSOCKET));
        config.set_string("device_name", Some("  "));
        config.set_number("initial_volume", Some(70));
        config.save().unwrap();

        let again = Config::load_from(&path).unwrap();
        assert_eq!(again.websocket().as_deref(), Some(DEFAULT_WEBSOCKET));
        assert_eq!(again.device_name(), None);
        assert_eq!(again.number("initial_volume"), Some(70));
        assert_eq!(again.values.get("future_key"), Some(&Value::Bool(true)));
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn service_states_parse() {
        assert_eq!(
            parse_service_state("LoadState=loaded\nActiveState=active\n"),
            ServiceState::Running
        );
        assert_eq!(
            parse_service_state("LoadState=not-found\nActiveState=inactive\n"),
            ServiceState::NotInstalled
        );
        assert_eq!(
            parse_service_state("LoadState=loaded\nActiveState=failed\n"),
            ServiceState::Failed
        );
        assert_eq!(
            parse_service_state("ActiveState=inactive\nLoadState=loaded\n"),
            ServiceState::Stopped
        );
    }
}
