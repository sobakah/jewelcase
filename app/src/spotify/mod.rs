//! The Spotify account (CONCEPT.md §9.3, §9.5, §9.6): logging in with the
//! user's own client ID, syncing playlists both ways and controlling
//! Spotify Connect devices.
//!
//! Network calls run on worker threads; the database and the UI stay on
//! the main thread.

mod keyring;
mod library;
pub mod preferences;
mod remote;
mod soloist;
mod soloistd_prefs;
mod stream;
mod sync;

use std::cell::{Cell, OnceCell, RefCell};
use std::sync::Arc;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use adw::prelude::*;
use gatefold_spotify::auth::{self, Callback, Pkce};
use gatefold_spotify::{Client, Error, Tokens};
use gettextrs::gettext;
use gtk::glib::subclass::Signal;
use gtk::subclass::prelude::*;
use gtk::{gio, glib};

use crate::application::GatefoldApplication;
use crate::config::APP_ID;

pub use remote::RemoteCommand;
pub use stream::{StreamNote, fetch_covers, fetch_covers_in_batches, image_cover_key};
pub use sync::{Op, answer_error, parse_suggestion, search_error};

/// How long the browser has to come back.
const LOGIN_TIMEOUT: Duration = Duration::from_secs(300);

/// "disconnected", "connecting" or "connected".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Disconnected,
    Connecting,
    Connected,
}

impl Status {
    fn as_str(self) -> &'static str {
        match self {
            Self::Disconnected => "disconnected",
            Self::Connecting => "connecting",
            Self::Connected => "connected",
        }
    }
}

mod imp {
    use super::*;

    #[derive(Default, glib::Properties)]
    #[properties(wrapper_type = super::Spotify)]
    pub struct Spotify {
        /// See `Status`.
        #[property(get)]
        pub status: RefCell<String>,
        /// The account's display name while connected.
        #[property(get, nullable)]
        pub account_name: RefCell<Option<String>>,
        /// Playlists are being read or changes sent.
        #[property(get)]
        pub syncing: Cell<bool>,
        /// The playlist whose changes are on their way to Spotify (0:
        /// none); it can't be edited meanwhile.
        #[property(get)]
        pub sending: Cell<i64>,
        /// Streaming (§9.4): "off", "connecting", "ready" or "error".
        #[property(get)]
        pub stream_status: RefCell<String>,
        pub stream: RefCell<stream::StreamState>,

        pub settings: OnceCell<gio::Settings>,
        pub client: RefCell<Option<Arc<Client>>>,
        pub user_id: RefCell<Option<String>>,
        pub cancel_login: RefCell<Option<Arc<AtomicBool>>>,
        pub sync: RefCell<sync::SyncState>,
        pub remote: RefCell<remote::RemoteState>,
        pub library: RefCell<library::LibraryState>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for Spotify {
        const NAME: &'static str = "GatefoldSpotify";
        type Type = super::Spotify;
    }

    #[glib::derived_properties]
    impl ObjectImpl for Spotify {
        fn signals() -> &'static [Signal] {
            static SIGNALS: OnceLock<Vec<Signal>> = OnceLock::new();
            SIGNALS.get_or_init(|| {
                vec![
                    // A message for the user (toast).
                    Signal::builder("message")
                        .param_types([String::static_type()])
                        .build(),
                    // Saved albums or followed artists changed, or their
                    // covers arrived (§9.8).
                    Signal::builder("library-changed").build(),
                ]
            })
        }

        fn constructed(&self) {
            self.parent_constructed();
            self.status
                .replace(Status::Disconnected.as_str().to_owned());
            self.stream_status.replace("off".to_owned());
            self.settings
                .set(gio::Settings::new(APP_ID))
                .expect("set once");
        }
    }
}

glib::wrapper! {
    pub struct Spotify(ObjectSubclass<imp::Spotify>);
}

impl Default for Spotify {
    fn default() -> Self {
        glib::Object::new()
    }
}

impl Spotify {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn settings(&self) -> &gio::Settings {
        self.imp().settings.get().expect("set in constructed()")
    }

    pub fn is_connected(&self) -> bool {
        self.imp().client.borrow().is_some()
    }

    pub(crate) fn client(&self) -> Option<Arc<Client>> {
        self.imp().client.borrow().clone()
    }

    /// The Spotify user ID, to tell own playlists from others'.
    pub(crate) fn user_id(&self) -> Option<String> {
        self.imp().user_id.borrow().clone()
    }

    /// The rate limits kept from earlier runs (see `use_tokens`).
    fn saved_blocks(&self) -> Vec<(String, std::time::SystemTime)> {
        self.settings()
            .get::<std::collections::HashMap<String, i64>>("spotify-blocked")
            .into_iter()
            .filter_map(|(kind, until)| {
                let until = std::time::UNIX_EPOCH
                    + std::time::Duration::from_secs(u64::try_from(until).ok()?);
                Some((kind, until))
            })
            .collect()
    }

    fn client_id(&self) -> String {
        self.settings()
            .string("spotify-client-id")
            .trim()
            .to_owned()
    }

    fn set_status(&self, status: Status) {
        let value = status.as_str();
        if *self.imp().status.borrow() != value {
            self.imp().status.replace(value.to_owned());
            self.notify_status();
        }
    }

    pub(crate) fn message(&self, text: &str) {
        self.emit_by_name::<()>("message", &[&text]);
    }

    /// On start: reconnects with the stored logins.
    pub fn start(&self) {
        self.start_streaming();
        self.start_account();
    }

    /// The Web API login only (retried while Spotify can't be reached;
    /// streaming keeps running).
    fn start_account(&self) {
        if self.settings().string("spotify-account").is_empty() || self.client_id().is_empty() {
            return;
        }
        glib::spawn_future_local(glib::clone!(
            #[weak(rename_to = spotify)]
            self,
            async move {
                match keyring::load().await {
                    Ok(Some(tokens)) => spotify.use_tokens(tokens, false).await,
                    Ok(None) => {
                        tracing::info!("No Spotify login in the keyring");
                        let _ = spotify.settings().set_string("spotify-account", "");
                    }
                    Err(err) => tracing::warn!("Cannot read the Spotify login: {err}"),
                }
            }
        ));
    }

    /// Logs in through the browser (§9.3).
    pub async fn connect(&self) {
        let client_id = self.client_id();
        if client_id.is_empty() {
            self.message(&gettext("Enter your Spotify client ID first"));
            return;
        }
        if self.imp().cancel_login.borrow().is_some() {
            return;
        }
        let callback = match Callback::bind() {
            Ok(callback) => callback,
            Err(err) => {
                tracing::warn!("Cannot listen for the Spotify login: {err}");
                // Translators: {} is a port number.
                self.message(
                    &gettext("Port {} is in use; close what uses it and try again")
                        .replace("{}", &auth::REDIRECT_PORT.to_string()),
                );
                return;
            }
        };
        self.set_status(Status::Connecting);
        let pkce = Pkce::new();
        let url = auth::authorize_url(&client_id, &pkce);
        open_in_browser(&url);
        // Development aid: with the stand-in server, "the browser" comes
        // back by itself.
        if cfg!(debug_assertions) && std::env::var_os("GATEFOLD_SPOTIFY_TEST_SERVER").is_some() {
            let state = pkce.state.clone();
            std::thread::spawn(move || {
                use std::io::Write;
                std::thread::sleep(Duration::from_millis(300));
                if let Ok(mut stream) =
                    std::net::TcpStream::connect(("127.0.0.1", auth::REDIRECT_PORT))
                {
                    let _ = write!(
                        stream,
                        "GET /callback?code=test&state={state} HTTP/1.1\r\nHost: x\r\n\r\n"
                    );
                    let _ = std::io::Read::read_to_end(&mut stream, &mut Vec::new());
                }
            });
        }

        let cancel = Arc::new(AtomicBool::new(false));
        self.imp().cancel_login.replace(Some(cancel.clone()));
        let state = pkce.state.clone();
        let code = gio::spawn_blocking(move || callback.wait(&state, LOGIN_TIMEOUT, &cancel))
            .await
            .unwrap_or(Err(Error::Cancelled));
        self.imp().cancel_login.replace(None);
        let tokens = match code {
            Ok(code) => gio::spawn_blocking(move || {
                gatefold_spotify::exchange_code(&client_id, &code, &pkce)
            })
            .await
            .unwrap_or(Err(Error::Cancelled)),
            Err(err) => Err(err),
        };
        match tokens {
            Ok(tokens) => self.use_tokens(tokens, true).await,
            Err(Error::Cancelled) => self.set_status(Status::Disconnected),
            Err(err) => {
                tracing::warn!("Spotify login failed: {err}");
                self.set_status(Status::Disconnected);
                self.message(&gettext("Couldn't connect to Spotify"));
            }
        }
    }

    pub fn cancel_connect(&self) {
        if let Some(cancel) = self.imp().cancel_login.borrow().as_ref() {
            cancel.store(true, Ordering::Relaxed);
        }
    }

    /// Starts the client, checks the account and reads the playlists.
    async fn use_tokens(&self, tokens: Tokens, fresh_login: bool) {
        let (sender, receiver) = async_channel::unbounded::<Tokens>();
        let (block_sender, blocks) = async_channel::unbounded::<(String, i64)>();
        let client = Arc::new(
            Client::new(&self.client_id(), tokens.clone(), move |t| {
                let _ = sender.send_blocking(t.clone());
            })
            .with_blocks(self.saved_blocks(), move |kind, until| {
                let _ = block_sender.send_blocking((kind.to_owned(), unix_seconds(until)));
            }),
        );
        // Rate limits Spotify announces are kept across restarts: asking
        // again during one could make it longer.
        let settings = self.settings().clone();
        glib::spawn_future_local(async move {
            while let Ok((kind, until)) = blocks.recv().await {
                let now = unix_seconds(std::time::SystemTime::now());
                let mut kept: std::collections::HashMap<String, i64> = settings
                    .get::<std::collections::HashMap<String, i64>>("spotify-blocked")
                    .into_iter()
                    .filter(|(_, until)| *until > now)
                    .collect();
                kept.insert(kind, until);
                if let Err(err) = settings.set("spotify-blocked", kept) {
                    tracing::warn!("Cannot keep Spotify's rate limit: {err}");
                }
            }
        });
        // Refreshed tokens go back into the keyring.
        glib::spawn_future_local(async move {
            while let Ok(tokens) = receiver.recv().await {
                if let Err(err) = keyring::save(tokens).await {
                    tracing::warn!("Cannot store the Spotify login: {err}");
                }
            }
        });
        let check = client.clone();
        let me = gio::spawn_blocking(move || check.me())
            .await
            .unwrap_or(Err(Error::Cancelled));
        let user = match me {
            Ok(user) => user,
            Err(Error::LoggedOut) => {
                self.logged_out();
                return;
            }
            Err(err) => {
                tracing::warn!("Cannot reach Spotify: {err}");
                // Offline at start: try again later with the same login.
                self.set_status(Status::Disconnected);
                if fresh_login {
                    self.message(&gettext("Couldn't connect to Spotify"));
                } else {
                    self.retry_start_later();
                }
                return;
            }
        };
        let name = user.display_name.clone().unwrap_or_else(|| user.id.clone());
        self.imp().user_id.replace(Some(user.id));
        self.imp().account_name.replace(Some(name.clone()));
        self.notify_account_name();
        self.imp().client.replace(Some(client));
        let _ = self.settings().set_string("spotify-account", &name);
        self.set_status(Status::Connected);
        if fresh_login {
            // Translators: {} is a Spotify account name.
            self.message(&gettext("Connected to Spotify as {}").replace("{}", &name));
            // In the background: the keyring may ask to be unlocked.
            glib::spawn_future_local(glib::clone!(
                #[weak(rename_to = spotify)]
                self,
                async move {
                    if let Err(err) = keyring::save(tokens).await {
                        tracing::warn!("Cannot store the Spotify login: {err}");
                        if keyring::disabled() {
                            return;
                        }
                        spotify.message(&gettext(
                            "The Spotify login couldn't be saved in the keyring; connect again after a restart",
                        ));
                    }
                }
            ));
        }
        self.sync_soon();
    }

    fn retry_start_later(&self) {
        glib::timeout_add_seconds_local_once(
            60,
            glib::clone!(
                #[weak(rename_to = spotify)]
                self,
                move || {
                    if !spotify.is_connected() {
                        spotify.start_account();
                    }
                }
            ),
        );
    }

    /// Logs out; Spotify mirrors are removed, published playlists stay.
    pub async fn disconnect(&self) {
        self.play_here(false);
        self.imp().client.replace(None);
        self.imp().user_id.replace(None);
        self.imp().account_name.replace(None);
        self.notify_account_name();
        let _ = self.settings().set_string("spotify-account", "");
        self.set_status(Status::Disconnected);
        if let Err(err) = keyring::delete().await {
            tracing::warn!("Cannot remove the Spotify login: {err}");
        }
        let library = GatefoldApplication::get().library();
        if let Some(mut db) = library.db()
            && let Err(err) = db.forget_spotify()
        {
            tracing::warn!("Cannot remove the Spotify playlists: {err}");
        }
        library.reload_playlists();
    }

    /// The login was revoked or expired.
    fn logged_out(&self) {
        self.play_here(false);
        self.imp().client.replace(None);
        self.imp().account_name.replace(None);
        self.notify_account_name();
        self.set_status(Status::Disconnected);
        self.message(&gettext(
            "The Spotify login expired; connect again in Preferences",
        ));
    }

    /// Runs `job` with the client on a worker thread. A lost login logs
    /// out.
    pub(crate) async fn call<T: Send + 'static>(
        &self,
        job: impl FnOnce(&Client) -> gatefold_spotify::Result<T> + Send + 'static,
    ) -> gatefold_spotify::Result<T> {
        let Some(client) = self.client() else {
            return Err(Error::LoggedOut);
        };
        let result = gio::spawn_blocking(move || job(&client))
            .await
            .unwrap_or(Err(Error::Cancelled));
        if matches!(result, Err(Error::LoggedOut)) && self.is_connected() {
            self.logged_out();
        }
        result
    }
}

/// Something on Spotify (`kind`: "artist", "album", "track", "playlist"):
/// in the Spotify app when that's chosen in Preferences and installed,
/// else on the web.
pub fn open_on_spotify(kind: &str, id: &str) {
    let in_app = GatefoldApplication::get()
        .player()
        .settings()
        .boolean("spotify-links-in-app");
    let uri = if in_app && gio::AppInfo::default_for_uri_scheme("spotify").is_some() {
        format!("spotify:{kind}:{id}")
    } else {
        format!("https://open.spotify.com/{kind}/{id}")
    };
    open_in_browser(&uri);
}

/// Opens a page in the browser (or, in headless tests, only logs it).
pub(crate) fn open_in_browser(url: &str) {
    if std::env::var_os("GATEFOLD_NO_BROWSER").is_some() {
        tracing::info!("Browser page (not opened): {url}");
        return;
    }
    let window = GatefoldApplication::get().active_window();
    gtk::UriLauncher::new(url).launch(window.as_ref(), gio::Cancellable::NONE, |result| {
        if let Err(err) = result {
            tracing::warn!("Cannot open the browser: {err}");
        }
    });
}

/// Unix seconds of a time (0 before 1970).
fn unix_seconds(time: std::time::SystemTime) -> i64 {
    time.duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64)
}
