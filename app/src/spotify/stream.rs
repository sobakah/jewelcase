//! Streaming from Spotify (CONCEPT.md §9.4): optional, unofficial
//! (librespot), built only with the `spotify-stream` feature. Spotify
//! songs that aren't in the library play through Gatefold's own output.
//!
//! Without the feature, everything here says "can't stream", so callers
//! don't need their own `cfg`s.

use gtk::subclass::prelude::*;

use super::Spotify;
use crate::application::GatefoldApplication;

/// What the stream tells the player.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StreamNote {
    Playing,
    Paused,
    Ended,
    /// Spotify can't play the song (region, removed, not logged in).
    Unavailable,
}

#[derive(Default)]
pub struct StreamState {
    #[cfg(feature = "spotify-stream")]
    player: Option<gatefold_stream::StreamPlayer>,
    ready: bool,
    /// The login just happened in the browser: store it once connected.
    #[cfg(feature = "spotify-stream")]
    fresh_login: bool,
}

impl Spotify {
    /// This build can stream at all.
    pub fn streaming_built() -> bool {
        cfg!(feature = "spotify-stream")
    }

    /// Streaming is on and logged in.
    pub fn can_stream(&self) -> bool {
        self.imp().stream.borrow().ready
    }

    /// Downloads the covers of streamed songs into the cover cache, in the
    /// background (their cache keys are known in advance).
    pub fn fetch_stream_covers(&self, ids: Vec<i64>) {
        let library = GatefoldApplication::get().library();
        let wanted = library
            .db()
            .and_then(|db| db.stream_covers(&ids).ok())
            .unwrap_or_default();
        if wanted.is_empty() {
            return;
        }
        gtk::glib::spawn_future_local(async move {
            let made = gtk::gio::spawn_blocking(move || {
                let covers =
                    gatefold_core::library::covers::CoverCache::new(crate::config::cover_dir());
                let mut made = 0;
                for (_, url) in wanted {
                    let key = gatefold_core::library::covers::cover_key('s', url.as_bytes());
                    if covers.has(&key) {
                        continue;
                    }
                    match gatefold_spotify::download_image(&url) {
                        Ok(data) => made += usize::from(covers.generate(&key, data).is_ok()),
                        Err(err) => tracing::info!("Cannot load a cover: {err}"),
                    }
                }
                made
            })
            .await
            .unwrap_or(0);
            // Shown before they arrived: show them now.
            if made > 0 {
                GatefoldApplication::get().player().covers_arrived();
            }
        });
    }

    fn set_stream_status(&self, status: &str) {
        if *self.imp().stream_status.borrow() != status {
            self.imp().stream_status.replace(status.to_owned());
            self.notify_stream_status();
        }
        self.imp().stream.borrow_mut().ready = status == "ready";
    }
}

#[cfg(feature = "spotify-stream")]
mod enabled {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::Duration;

    use gatefold_spotify::Error;
    use gatefold_spotify::auth::{self, Callback, Pkce};
    use gatefold_stream::{Credentials, Login, StreamEvent, StreamPlayer};
    use gettextrs::gettext;
    use gtk::glib;
    use gtk::prelude::*;

    use super::*;
    use crate::spotify::keyring;

    impl Spotify {
        /// On start: logs in with the stored streaming login.
        pub fn start_streaming(&self) {
            if !self.settings().boolean("spotify-streaming") {
                return;
            }
            glib::spawn_future_local(glib::clone!(
                #[weak(rename_to = spotify)]
                self,
                async move {
                    match keyring::load_stream().await {
                        Ok(Some(text)) => match Credentials::from_json(&text) {
                            Some(credentials) => {
                                spotify.stream_connect(Login::Stored(credentials), false)
                            }
                            None => spotify.set_stream_status("off"),
                        },
                        Ok(None) => spotify.set_stream_status("off"),
                        Err(err) => {
                            tracing::warn!("Cannot read the streaming login: {err}");
                            spotify.set_stream_status("off");
                        }
                    }
                }
            ));
        }

        /// Turns streaming on: a login in the browser (as librespot, the
        /// only client Spotify lets stream), then connects.
        pub async fn enable_streaming(&self) {
            let callback = match Callback::bind_at(gatefold_stream::REDIRECT_PORT, "/login") {
                Ok(callback) => callback,
                Err(err) => {
                    tracing::warn!("Cannot listen for the streaming login: {err}");
                    // Translators: {} is a port number.
                    self.message(
                        &gettext("Port {} is in use; close what uses it and try again")
                            .replace("{}", &gatefold_stream::REDIRECT_PORT.to_string()),
                    );
                    let _ = self.settings().set_boolean("spotify-streaming", false);
                    return;
                }
            };
            self.set_stream_status("connecting");
            let pkce = Pkce::new();
            let url = auth::authorize_url_for(
                gatefold_stream::CLIENT_ID,
                gatefold_stream::REDIRECT_URI,
                gatefold_stream::SCOPES,
                &pkce,
            );
            super::super::open_in_browser(&url);
            let cancel = Arc::new(AtomicBool::new(false));
            // Turning the switch off again cancels the login.
            let watch = self.settings().connect_changed(
                Some("spotify-streaming"),
                glib::clone!(
                    #[strong]
                    cancel,
                    move |settings, _| {
                        if !settings.boolean("spotify-streaming") {
                            cancel.store(true, Ordering::Relaxed);
                        }
                    }
                ),
            );
            let state = pkce.state.clone();
            let code = gtk::gio::spawn_blocking(move || {
                callback.wait(&state, Duration::from_secs(300), &cancel)
            })
            .await
            .unwrap_or(Err(Error::Cancelled));
            self.settings().disconnect(watch);
            let token = match code {
                Ok(code) => gtk::gio::spawn_blocking(move || {
                    gatefold_spotify::exchange_code_for(
                        gatefold_stream::CLIENT_ID,
                        gatefold_stream::REDIRECT_URI,
                        &code,
                        &pkce,
                    )
                })
                .await
                .unwrap_or(Err(Error::Cancelled)),
                Err(err) => Err(err),
            };
            match token {
                Ok(tokens) => self.stream_connect(Login::Token(tokens.access_token), true),
                Err(err) => {
                    if !matches!(err, Error::Cancelled) {
                        tracing::warn!("Streaming login failed: {err}");
                        self.message(&gettext("Couldn't log in for streaming"));
                    }
                    self.set_stream_status("off");
                    let _ = self.settings().set_boolean("spotify-streaming", false);
                }
            }
        }

        /// Turns streaming off and forgets its login.
        pub async fn disable_streaming(&self) {
            if let Some(player) = self.imp().stream.borrow().player.as_ref() {
                player.stop();
            }
            self.imp().stream.borrow_mut().player = None;
            self.set_stream_status("off");
            GatefoldApplication::get().player().stream_gone();
            if let Err(err) = keyring::delete_stream().await {
                tracing::info!("Cannot remove the streaming login: {err}");
            }
        }

        fn stream_connect(&self, login: Login, fresh: bool) {
            self.set_stream_status("connecting");
            if self.imp().stream.borrow().player.is_none() {
                let silent = std::env::var_os("GATEFOLD_SILENT").is_some();
                match StreamPlayer::new(silent) {
                    Ok((player, events)) => {
                        player.set_volume(GatefoldApplication::get().player().volume());
                        self.imp().stream.borrow_mut().player = Some(player);
                        glib::spawn_future_local(glib::clone!(
                            #[weak(rename_to = spotify)]
                            self,
                            async move {
                                while let Ok(event) = events.recv().await {
                                    spotify.stream_event(event).await;
                                }
                            }
                        ));
                    }
                    Err(err) => {
                        tracing::warn!("Streaming is unavailable: {err}");
                        self.set_stream_status("off");
                        return;
                    }
                }
            }
            self.imp().stream.borrow_mut().fresh_login = fresh;
            let normalise = self.settings().string("replaygain") != "off";
            if let Some(player) = self.imp().stream.borrow().player.as_ref() {
                player.connect(login, normalise);
            }
        }

        async fn stream_event(&self, event: StreamEvent) {
            let player = GatefoldApplication::get().player();
            match event {
                StreamEvent::Connected(credentials) => {
                    tracing::info!("Streaming as {}", credentials.username);
                    self.set_stream_status("ready");
                    if std::mem::take(&mut self.imp().stream.borrow_mut().fresh_login) {
                        self.message(&gettext(
                            "Songs that aren't in your library now play from Spotify",
                        ));
                        if let Err(err) = keyring::save_stream(credentials.to_json()).await
                            && !keyring::disabled()
                        {
                            tracing::warn!("Cannot store the streaming login: {err}");
                        }
                    }
                }
                StreamEvent::ConnectFailed(err) => {
                    tracing::warn!("Streaming login failed: {err}");
                    self.set_stream_status("error");
                    self.message(&gettext("Streaming from Spotify isn't working right now"));
                }
                StreamEvent::Playing { .. } => player.stream_note(StreamNote::Playing),
                StreamEvent::Paused { .. } => player.stream_note(StreamNote::Paused),
                StreamEvent::EndOfTrack => player.stream_note(StreamNote::Ended),
                StreamEvent::Unavailable => player.stream_note(StreamNote::Unavailable),
            }
        }

        fn with_stream(&self, f: impl FnOnce(&StreamPlayer)) {
            if let Some(player) = self.imp().stream.borrow().player.as_ref() {
                f(player);
            }
        }

        pub fn stream_load(&self, uri: &str, start_ms: u64, play: bool) {
            self.with_stream(|p| p.load(uri, start_ms, play));
        }

        pub fn stream_preload(&self, uri: &str) {
            self.with_stream(|p| p.preload(uri));
        }

        pub fn stream_play(&self) {
            self.with_stream(StreamPlayer::play);
        }

        pub fn stream_pause(&self) {
            self.with_stream(StreamPlayer::pause);
        }

        pub fn stream_stop(&self) {
            self.with_stream(StreamPlayer::stop);
        }

        pub fn stream_seek(&self, ms: u64) {
            self.with_stream(|p| p.seek(ms));
        }

        pub fn stream_volume(&self, volume: f64) {
            self.with_stream(|p| p.set_volume(volume));
        }

        pub fn stream_position(&self) -> u64 {
            self.imp()
                .stream
                .borrow()
                .player
                .as_ref()
                .map_or(0, StreamPlayer::position_ms)
        }
    }
}

#[cfg(not(feature = "spotify-stream"))]
impl Spotify {
    pub fn start_streaming(&self) {}
    pub async fn enable_streaming(&self) {
        self.set_stream_status("off");
    }
    pub async fn disable_streaming(&self) {}
    pub fn stream_load(&self, _uri: &str, _start_ms: u64, _play: bool) {
        GatefoldApplication::get()
            .player()
            .stream_note(StreamNote::Unavailable);
    }
    pub fn stream_preload(&self, _uri: &str) {}
    pub fn stream_play(&self) {}
    pub fn stream_pause(&self) {}
    pub fn stream_stop(&self) {}
    pub fn stream_seek(&self, _ms: u64) {}
    pub fn stream_volume(&self, _volume: f64) {}
    pub fn stream_position(&self) -> u64 {
        0
    }
}
