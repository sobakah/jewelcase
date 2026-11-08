//! Spotify Soloist (CONCEPT.md §9.4): Spotify's official headless Connect
//! client. Gatefold starts it with the user's own API key and drives it over
//! its local WebSocket API (bound to 127.0.0.1); Soloist plays the audio
//! itself.
//!
//! Soloist isn't shipped with Gatefold (Spotify doesn't allow that): the
//! user downloads it from Spotify. Builds expire after 90 days.
//!
//! Threads: one reads Soloist's output into the log, one waits for it to
//! exit, one talks WebSocket. Events arrive on an async channel; commands
//! never block the caller.

use std::io::{BufRead, BufReader};
use std::net::TcpStream;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};

pub mod soloistd;
use tungstenite::{Message, WebSocket};

/// Where users get Soloist and their API key.
pub const DOWNLOADS_URL: &str =
    "https://developer.spotify.com/documentation/soloist/reference/downloads-and-updates";
pub const API_KEY_URL: &str = "https://developer.spotify.com/dashboard/soloist";
/// Soloist's exit code for an expired build.
pub const EXIT_EXPIRED: i32 = 10;

const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
const READ_POLL: Duration = Duration::from_millis(50);

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("Soloist couldn't be started: {0}")]
    Start(#[from] std::io::Error),
}

/// How to start Soloist.
#[derive(Debug, Clone)]
pub struct Config {
    pub binary: PathBuf,
    pub api_key: String,
    /// The name in Spotify's device lists.
    pub device_name: String,
    /// Gatefold's own data and cache folders, so the user's own Soloist
    /// (if any) stays separate.
    pub data_dir: PathBuf,
    pub cache_dir: PathBuf,
    /// 0 to 100.
    pub initial_volume: u8,
}

/// Soloist on `PATH`, in `~/.local/bin` or `/usr/local/bin`, or the given
/// path.
pub fn find_binary(configured: Option<&Path>) -> Option<PathBuf> {
    if let Some(path) = configured {
        return path.is_file().then(|| path.to_path_buf());
    }
    let mut candidates: Vec<PathBuf> = std::env::var_os("PATH")
        .map(|path| {
            std::env::split_paths(&path)
                .map(|d| d.join("soloist"))
                .collect()
        })
        .unwrap_or_default();
    if let Some(home) = std::env::var_os("HOME") {
        candidates.push(PathBuf::from(home).join(".local/bin/soloist"));
    }
    candidates.push(PathBuf::from("/usr/local/bin/soloist"));
    candidates.into_iter().find(|p| p.is_file())
}

/// `soloist --version`'s first line, e.g. its version and build date.
pub fn version(binary: &Path) -> Option<String> {
    let output = Command::new(binary)
        .arg("--version")
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .find(|l| !l.trim().is_empty())
        .map(|l| l.trim().to_owned())
}

/// A song as Soloist describes it.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Item {
    pub uri: String,
    pub name: String,
    pub artists: Vec<String>,
    pub album: Option<String>,
    pub duration_ms: Option<u64>,
    /// The largest cover.
    pub cover_url: Option<String>,
}

impl Item {
    fn parse(value: &Value) -> Option<Self> {
        let uri = value["uri"].as_str()?.to_owned();
        let decorations = &value["decorations"];
        let name = |v: &Value| {
            v["decorations"]["identity"]["name"]
                .as_str()
                .map(str::to_owned)
        };
        let covers = decorations["visual_identity"]["cover"].as_array();
        // The largest: covers are shown large.
        let cover_url = covers.and_then(|covers| {
            ["xlarge", "large", "default", "small"]
                .iter()
                .find_map(|size| covers.iter().find(|c| c["size"].as_str() == Some(size)))
                .or_else(|| covers.first())
                .and_then(|c| c["url"].as_str().map(str::to_owned))
        });
        Some(Self {
            uri,
            name: decorations["identity"]["name"]
                .as_str()
                .unwrap_or_default()
                .to_owned(),
            artists: decorations["creators"]
                .as_array()
                .map(|creators| creators.iter().filter_map(|c| name(&c["entity"])).collect())
                .unwrap_or_default(),
            album: name(&decorations["parent"]["entity"]),
            duration_ms: decorations["playback"]["duration_ms"].as_u64(),
            cover_url,
        })
    }
}

/// Where playback was at a moment (Soloist's position anchor).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Position {
    pub position_ms: u64,
    /// Unix milliseconds.
    pub timestamp_ms: u64,
    pub speed: f64,
}

impl Position {
    fn parse(value: &Value) -> Option<Self> {
        Some(Self {
            position_ms: value["position_ms"].as_u64()?,
            timestamp_ms: value["timestamp_ms"].as_u64().unwrap_or_else(now_ms),
            speed: value["speed"].as_f64().unwrap_or(1.0),
        })
    }

    /// The position now.
    pub fn now(&self) -> u64 {
        let elapsed = now_ms().saturating_sub(self.timestamp_ms) as f64 * self.speed;
        self.position_ms + elapsed.max(0.0) as u64
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Idle,
    Playing,
    Paused,
    Buffering,
}

impl Status {
    fn parse(value: &Value) -> Option<Self> {
        Some(match value.as_str()? {
            "idle" => Self::Idle,
            "playing" => Self::Playing,
            "paused" => Self::Paused,
            "buffering" => Self::Buffering,
            _ => return None,
        })
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    /// The WebSocket is connected.
    Connected,
    Auth {
        logged_in: bool,
        is_active: bool,
        /// The name in Spotify's device lists.
        device_name: Option<String>,
    },
    /// A full snapshot.
    Playback {
        status: Option<Status>,
        item: Option<Item>,
        position: Option<Position>,
        volume: Option<u8>,
        is_active: Option<bool>,
        options: Option<Options>,
    },
    /// Shuffle or repeat changed (in Gatefold or the Spotify app).
    Options(Options),
    /// Soloist became (or stopped being) the active Connect device.
    Device {
        is_active: bool,
    },
    TrackChanged(Option<Item>),
    Status(Status),
    Position(Position),
    Volume(u8),
    Queue(Vec<Item>),
    /// An error answer to a command.
    CommandError {
        /// The control command it answers, when known.
        command: Option<String>,
        message: String,
    },
    /// Soloist stopped; `expired`: its build is too old.
    Exited {
        code: Option<i32>,
        expired: bool,
    },
    /// The WebSocket closed (a Soloist Gatefold didn't start went away).
    Disconnected,
}

impl Event {
    /// A server message, or `None` for ones Gatefold doesn't need.
    pub fn parse(text: &str) -> Option<Self> {
        let value: Value = serde_json::from_str(text).ok()?;
        let volume = |v: &Value| v.as_u64().map(|n| n.min(100) as u8);
        Some(match value["type"].as_str()? {
            "auth_state" => Self::Auth {
                logged_in: value["logged_in"].as_bool().unwrap_or(false),
                is_active: value["is_active"].as_bool().unwrap_or(false),
                device_name: value["device_name"].as_str().map(str::to_owned),
            },
            "playback_state" => Self::Playback {
                status: Status::parse(&value["status"]),
                item: Item::parse(&value["item"]),
                position: Position::parse(&value["position"]),
                volume: volume(&value["volume"]),
                is_active: value["is_active"].as_bool(),
                options: Options::parse(&value["options"]),
            },
            "options_changed" => Self::Options(Options::parse(&value["options"])?),
            "device_changed" => Self::Device {
                is_active: value["is_active"].as_bool().unwrap_or(false),
            },
            "track_changed" => Self::TrackChanged(Item::parse(&value["item"])),
            "playback_changed" => Self::Status(Status::parse(&value["status"])?),
            "position_sync" => Self::Position(Position::parse(&value["position"])?),
            "volume_changed" => Self::Volume(volume(&value["volume"])?),
            "queue_changed" => Self::Queue(
                value["upcoming"]
                    .as_array()
                    .map(|items| {
                        items
                            .iter()
                            .filter_map(|e| Item::parse(&e["item"]))
                            .collect()
                    })
                    .unwrap_or_default(),
            ),
            "error" => Self::CommandError {
                command: None,
                message: value["message"].as_str().unwrap_or_default().to_owned(),
            },
            _ => return None,
        })
    }
}

/// Repeat as Soloist knows it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Repeat {
    #[default]
    Off,
    Context,
    Track,
}

/// Shuffle and repeat, as Soloist reports them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Options {
    pub shuffle: bool,
    pub repeat: Repeat,
}

impl Options {
    fn parse(value: &Value) -> Option<Self> {
        if !value.is_object() {
            return None;
        }
        Some(Self {
            shuffle: value["shuffle"].as_bool().unwrap_or(false),
            repeat: match value["repeat"].as_str() {
                Some("context") => Repeat::Context,
                Some("track") => Repeat::Track,
                _ => Repeat::Off,
            },
        })
    }
}

/// A running Soloist.
pub struct Soloist {
    commands: mpsc::Sender<Value>,
    child: Arc<Mutex<Option<Child>>>,
    /// The WebSocket thread, to wait for it to send what's queued.
    thread: Mutex<Option<std::thread::JoinHandle<()>>>,
}

impl Soloist {
    /// Starts Soloist and connects to it. Events (including `Exited`) arrive
    /// on the returned channel.
    pub fn start(config: &Config) -> Result<(Self, async_channel::Receiver<Event>), Error> {
        std::fs::create_dir_all(&config.data_dir)?;
        std::fs::create_dir_all(&config.cache_dir)?;
        // A crashed run may have left these behind.
        for name in ["ws.port", "ws.addr"] {
            let _ = std::fs::remove_file(config.data_dir.join(name));
        }
        let mut command = Command::new(&config.binary);
        // If Gatefold dies without stopping it, so does Soloist (Linux).
        // SAFETY: prctl is async-signal-safe; nothing else runs here.
        unsafe {
            command.pre_exec(|| {
                libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGTERM);
                Ok(())
            });
        }
        let mut child = command
            .arg("--device-name")
            .arg(&config.device_name)
            .arg("--api-key")
            .arg(&config.api_key)
            .arg("--data-dir")
            .arg(&config.data_dir)
            .arg("--cache-dir")
            .arg(&config.cache_dir)
            .arg("--initial-volume")
            .arg(config.initial_volume.min(100).to_string())
            .arg("--ws")
            .arg("127.0.0.1:0")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;
        for pipe in [
            child
                .stdout
                .take()
                .map(|p| Box::new(p) as Box<dyn std::io::Read + Send>),
            child
                .stderr
                .take()
                .map(|p| Box::new(p) as Box<dyn std::io::Read + Send>),
        ]
        .into_iter()
        .flatten()
        {
            std::thread::spawn(move || {
                for line in BufReader::new(pipe).lines().map_while(Result::ok) {
                    tracing::info!(target: "soloist", "{line}");
                }
            });
        }
        let child = Arc::new(Mutex::new(Some(child)));
        let (events, receiver) = async_channel::unbounded();

        // Waits for the exit.
        {
            let child = child.clone();
            let events = events.clone();
            std::thread::spawn(move || {
                let code = loop {
                    let status = {
                        let mut guard = child.lock().unwrap_or_else(|e| e.into_inner());
                        match guard.as_mut() {
                            Some(child) => child.try_wait().ok().flatten(),
                            None => break None,
                        }
                    };
                    if let Some(status) = status {
                        break status.code();
                    }
                    std::thread::sleep(Duration::from_millis(300));
                };
                let _ = events.send_blocking(Event::Exited {
                    code,
                    expired: code == Some(EXIT_EXPIRED),
                });
            });
        }

        // Talks to it, once it has written its port.
        let data_dir = config.data_dir.clone();
        let (commands, thread) = talk(events, move || wait_for_port(&data_dir));
        Ok((
            Self {
                commands,
                child,
                thread: Mutex::new(Some(thread)),
            },
            receiver,
        ))
    }

    /// Uses a Soloist that already runs (e.g. kept running by soloistd),
    /// at its WebSocket address. Nothing is started or stopped.
    pub fn attach(addr: &str, port: u16) -> (Self, async_channel::Receiver<Event>) {
        let (events, receiver) = async_channel::unbounded();
        let addr = local_only(addr);
        let (commands, thread) = talk(events, move || Some((addr.clone(), port)));
        (
            Self {
                commands,
                child: Arc::new(Mutex::new(None)),
                thread: Mutex::new(Some(thread)),
            },
            receiver,
        )
    }

    fn command(&self, name: &str, fields: Value) {
        let mut command = json!({ "type": "command", "command": name });
        if let (Some(command), Some(fields)) = (command.as_object_mut(), fields.as_object()) {
            command.extend(fields.clone());
        }
        let _ = self.commands.send(command);
    }

    /// Starts a Spotify URI (track, album, playlist), or resumes.
    pub fn play(&self, uri: Option<&str>) {
        match uri {
            Some(uri) => self.command("play", json!({ "uri": uri })),
            None => self.command("play", json!({})),
        }
    }

    pub fn pause(&self) {
        self.command("pause", json!({}));
    }

    pub fn seek(&self, ms: u64) {
        self.command("seek", json!({ "position_ms": ms }));
    }

    /// 0 to 100.
    pub fn set_volume(&self, volume: u8) {
        self.command("set_volume", json!({ "volume": volume.min(100) }));
    }

    pub fn set_shuffle(&self, enabled: bool) {
        self.command("set_shuffle", json!({ "enabled": enabled }));
    }

    /// In the order Soloist's documentation gives.
    pub fn set_repeat(&self, repeat: Repeat) {
        let (first, first_on, second, second_on) = match repeat {
            Repeat::Off => ("set_repeat_track", false, "set_repeat_context", false),
            Repeat::Context => ("set_repeat_track", false, "set_repeat_context", true),
            Repeat::Track => ("set_repeat_context", false, "set_repeat_track", true),
        };
        self.command(first, json!({ "enabled": first_on }));
        self.command(second, json!({ "enabled": second_on }));
    }

    /// Adds a track after the current song.
    pub fn add_to_queue(&self, uri: &str) {
        self.command("add_to_queue", json!({ "uri": uri }));
    }

    pub fn skip_next(&self) {
        self.command("skip_next", json!({}));
    }

    pub fn skip_previous(&self) {
        self.command("skip_prev", json!({}));
    }

    /// Asks for the queue (answered with a `Queue` event).
    pub fn request_queue(&self, limit: u32) {
        self.command("get_queue", json!({ "limit": limit }));
    }

    /// Makes Soloist the active Spotify Connect device.
    pub fn activate(&self) {
        self.command("activate", json!({}));
    }

    pub fn request_state(&self) {
        self.command("get_state", json!({}));
    }

    /// A Soloist that was already running (not started here).
    /// Done with Soloist: what's queued is sent (a last pause), a Soloist
    /// Gatefold started stops; waits at most `max` for the sending.
    pub fn close(self, max: Duration) {
        let thread = self.thread.lock().unwrap_or_else(|e| e.into_inner()).take();
        // Dropping the command sender ends the thread after it sent the rest.
        drop(self);
        if let Some(thread) = thread {
            let start = Instant::now();
            while !thread.is_finished() && start.elapsed() < max {
                std::thread::sleep(Duration::from_millis(20));
            }
        }
    }

    pub fn is_attached(&self) -> bool {
        self.child.lock().map(|c| c.is_none()).unwrap_or(true)
    }

    /// Stops Soloist (Gatefold quits or streaming is turned off).
    pub fn shutdown(&self) {
        let child = self.child.lock().unwrap_or_else(|e| e.into_inner()).take();
        if let Some(mut child) = child {
            // SIGTERM first, so Soloist can remove its runtime files.
            if let Ok(pid) = libc::pid_t::try_from(child.id()) {
                // SAFETY: a plain signal to our own child.
                unsafe {
                    libc::kill(pid, libc::SIGTERM);
                }
            }
            let start = Instant::now();
            while start.elapsed() < Duration::from_secs(3) {
                if child.try_wait().ok().flatten().is_some() {
                    return;
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

impl Drop for Soloist {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// The WebSocket thread: connects (`target` may wait for the address),
/// then forwards events and sends commands. Returns the command sender
/// and the thread.
fn talk(
    events: async_channel::Sender<Event>,
    target: impl FnOnce() -> Option<(String, u16)> + Send + 'static,
) -> (mpsc::Sender<Value>, std::thread::JoinHandle<()>) {
    let (commands, command_receiver) = mpsc::channel::<Value>();
    let thread = std::thread::spawn(move || {
        let Some(mut socket) = target().and_then(|(addr, port)| connect_to(&addr, port)) else {
            tracing::warn!("Soloist's WebSocket API isn't reachable");
            let _ = events.send_blocking(Event::Disconnected);
            return;
        };
        let _ = events.send_blocking(Event::Connected);
        // Control commands waiting for their answer: Soloist answers each
        // with `command_result` or `error`, in order, and an error doesn't
        // say which command it's about.
        let mut unanswered: std::collections::VecDeque<String> = Default::default();
        loop {
            match socket.read() {
                Ok(Message::Text(text)) => {
                    if let Some(name) = command_result(&text) {
                        if let Some(i) = unanswered.iter().position(|c| *c == name) {
                            unanswered.remove(i);
                        }
                        continue;
                    }
                    let event = match Event::parse(&text) {
                        Some(Event::CommandError { message, .. }) => Some(Event::CommandError {
                            command: unanswered.pop_front(),
                            message,
                        }),
                        event => event,
                    };
                    if let Some(event) = event
                        && events.send_blocking(event).is_err()
                    {
                        return;
                    }
                }
                Ok(Message::Close(_)) => break,
                Ok(_) => {}
                Err(tungstenite::Error::Io(err))
                    if matches!(
                        err.kind(),
                        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                    ) => {}
                Err(err) => {
                    tracing::info!("Soloist's WebSocket closed: {err}");
                    break;
                }
            }
            loop {
                match command_receiver.try_recv() {
                    Ok(command) => {
                        if let Err(err) = socket.send(Message::text(command.to_string())) {
                            tracing::warn!("Cannot send to Soloist: {err}");
                        } else if let Some(name) = command["command"].as_str()
                            && !QUERIES.contains(&name)
                        {
                            unanswered.push_back(name.to_owned());
                        }
                    }
                    Err(mpsc::TryRecvError::Empty) => break,
                    Err(mpsc::TryRecvError::Disconnected) => {
                        let _ = socket.close(None);
                        return;
                    }
                }
            }
        }
        let _ = events.send_blocking(Event::Disconnected);
    });
    (commands, thread)
}

/// Commands answered with their event, not with `command_result`.
const QUERIES: [&str; 3] = ["get_auth_state", "get_state", "get_queue"];

/// The command a `command_result` answers.
fn command_result(text: &str) -> Option<String> {
    let value: Value = serde_json::from_str(text).ok()?;
    (value["type"] == "command_result").then(|| value["command"].as_str().map(str::to_owned))?
}

/// The API has no authentication: never talk to it over the network.
fn local_only(addr: &str) -> String {
    let addr = addr.trim();
    if addr.starts_with("127.") || addr == "localhost" || addr == "::1" {
        addr.to_owned()
    } else {
        "127.0.0.1".into()
    }
}

/// The address Soloist writes into its data folder (`ws.addr`, `ws.port`)
/// while it runs with `--ws`.
pub fn runtime_address(data_dir: &Path) -> Option<(String, u16)> {
    let port = std::fs::read_to_string(data_dir.join("ws.port"))
        .ok()?
        .trim()
        .parse()
        .ok()?;
    let addr = std::fs::read_to_string(data_dir.join("ws.addr"))
        .ok()
        .map(|a| a.trim().to_owned())
        .filter(|a| !a.is_empty())
        .unwrap_or_else(|| "127.0.0.1".into());
    Some((local_only(&addr), port))
}

/// Waits for a just-started Soloist to write its address.
fn wait_for_port(data_dir: &Path) -> Option<(String, u16)> {
    let start = Instant::now();
    loop {
        if let Some(address) = runtime_address(data_dir) {
            return Some(address);
        }
        if start.elapsed() > CONNECT_TIMEOUT {
            return None;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

fn connect_to(addr: &str, port: u16) -> Option<WebSocket<TcpStream>> {
    let start = Instant::now();
    loop {
        if let Ok(stream) = TcpStream::connect((addr, port)) {
            let url = format!("ws://{addr}:{port}");
            match tungstenite::client::client(url.as_str(), stream) {
                Ok((socket, _)) => {
                    let _ = socket.get_ref().set_read_timeout(Some(READ_POLL));
                    return Some(socket);
                }
                Err(err) => tracing::info!("Soloist handshake failed: {err}"),
            }
        }
        if start.elapsed() > CONNECT_TIMEOUT {
            return None;
        }
        std::thread::sleep(Duration::from_millis(200));
    }
}

/// A Soloist that's already running (CONCEPT.md §9.4): where it came from
/// and its WebSocket address.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Running {
    /// "soloistd", "soloist", or "gatefold" (one Gatefold started earlier).
    pub source: &'static str,
    pub addr: String,
    pub port: u16,
}

/// Finds the Soloist to use: soloistd's when its WebSocket API is on
/// (`websocket` in its config.json), else one running with Soloist's
/// default data folder (its runtime files).
pub fn find_running() -> Option<Running> {
    let home = std::env::var_os("HOME").map(PathBuf::from)?;
    let xdg = |var: &str, fallback: &str| {
        std::env::var_os(var)
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join(fallback))
    };
    // soloistd with its WebSocket on: its Soloist, even while it's just
    // restarting (connecting waits for it). Not while the service is
    // stopped, unless something answers there (`soloistd run` by hand).
    let config = soloistd_config();
    if let Some((addr, port)) = std::fs::read_to_string(&config)
        .ok()
        .and_then(|text| soloistd_websocket(&text))
        .filter(|(addr, port)| {
            use soloistd::ServiceState;
            matches!(
                soloistd::service_state(),
                ServiceState::Running | ServiceState::Starting | ServiceState::Unknown
            ) || std::net::TcpStream::connect_timeout(
                &std::net::SocketAddr::new(
                    addr.parse().unwrap_or(std::net::Ipv4Addr::LOCALHOST.into()),
                    *port,
                ),
                Duration::from_millis(300),
            )
            .is_ok()
        })
    {
        return Some(Running {
            source: "soloistd",
            addr,
            port,
        });
    }
    let data = xdg("XDG_DATA_HOME", ".local/share").join("soloist");
    runtime_address(&data).map(|(addr, port)| Running {
        source: "soloist",
        addr,
        port,
    })
}

/// soloistd's `websocket` setting ("127.0.0.1:9090"), local only.
pub fn soloistd_websocket(config_json: &str) -> Option<(String, u16)> {
    let value: Value = serde_json::from_str(config_json).ok()?;
    let (addr, port) = value["websocket"].as_str()?.rsplit_once(':')?;
    let addr = if addr.is_empty() || addr == "localhost" {
        "127.0.0.1"
    } else {
        addr
    };
    Some((local_only(addr), port.parse().ok()?))
}

fn soloistd_config() -> PathBuf {
    soloistd::config_path()
}

/// soloistd is set up (it manages Soloist's download and updates).
pub fn soloistd_installed() -> bool {
    soloistd_config().is_file()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn events_parse() {
        let snapshot = r#"{"type":"playback_state","status":"playing",
            "item":{"uri":"spotify:track:x","entity_type":"track","decorations":{
              "identity":{"name":"My Song"},
              "playback":{"duration_ms":210000},
              "creators":[{"entity":{"decorations":{"identity":{"name":"YEONJUN"}}}}],
              "parent":{"entity":{"decorations":{"identity":{"name":"NO LABELS"}}}},
              "visual_identity":{"cover":[{"url":"s","size":"small"},{"url":"d","size":"default"}]}}},
            "position":{"position_ms":45000,"timestamp_ms":1,"speed":1.0},
            "volume":65,"is_active":true}"#;
        let Some(Event::Playback {
            status,
            item: Some(item),
            position: Some(position),
            volume,
            is_active,
            ..
        }) = Event::parse(snapshot)
        else {
            panic!("not a snapshot");
        };
        assert_eq!(status, Some(Status::Playing));
        assert_eq!(item.name, "My Song");
        assert_eq!(item.artists, ["YEONJUN"]);
        assert_eq!(item.album.as_deref(), Some("NO LABELS"));
        assert_eq!(item.duration_ms, Some(210_000));
        assert_eq!(item.cover_url.as_deref(), Some("d"));
        assert_eq!(position.position_ms, 45_000);
        assert_eq!(volume, Some(65));
        assert_eq!(is_active, Some(true));
        assert_eq!(
            Event::parse(
                r#"{"type":"options_changed","options":{"shuffle":true,"repeat":"track","playback_speed":1.0}}"#
            ),
            Some(Event::Options(Options {
                shuffle: true,
                repeat: Repeat::Track
            }))
        );
        assert_eq!(
            Event::parse(r#"{"type":"device_changed","is_active":false,"device_name":"G"}"#),
            Some(Event::Device { is_active: false })
        );
        assert_eq!(
            Event::parse(
                r#"{"type":"auth_state","logged_in":false,"is_active":false,"device_name":"G"}"#
            ),
            Some(Event::Auth {
                logged_in: false,
                is_active: false,
                device_name: Some("G".into()),
            })
        );
        assert_eq!(
            Event::parse(r#"{"type":"playback_changed","status":"paused"}"#),
            Some(Event::Status(Status::Paused))
        );
        assert_eq!(
            Event::parse(r#"{"type":"error","message":"command requires authentication"}"#),
            Some(Event::CommandError {
                command: None,
                message: "command requires authentication".into()
            })
        );
        assert_eq!(
            command_result(r#"{"type":"command_result","command":"pause"}"#).as_deref(),
            Some("pause")
        );
        assert_eq!(
            Event::parse(r#"{"type":"command_result","command":"pause"}"#),
            None
        );
    }

    #[test]
    fn soloistd_config_gives_the_address() {
        assert_eq!(
            soloistd_websocket(r#"{"api_key":"k","websocket":"127.0.0.1:9090"}"#),
            Some(("127.0.0.1".into(), 9090))
        );
        assert_eq!(
            soloistd_websocket(r#"{"websocket":"0.0.0.0:9090"}"#),
            Some(("127.0.0.1".into(), 9090))
        );
        assert_eq!(soloistd_websocket(r#"{"api_key":"k"}"#), None);
    }

    #[test]
    fn positions_move_with_time() {
        let position = Position {
            position_ms: 1000,
            timestamp_ms: now_ms() - 500,
            speed: 1.0,
        };
        assert!(position.now() >= 1500);
        let paused = Position {
            speed: 0.0,
            ..position
        };
        assert_eq!(paused.now(), 1000);
    }
}
