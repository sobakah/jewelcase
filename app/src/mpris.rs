//! MPRIS for GNOME Shell's media controls and media keys (CONCEPT.md §11).

use gatefold_core::library::covers::CoverSize;
use gatefold_core::queue::RepeatMode;
use gtk::glib;
use gtk::prelude::*;
use mpris_server::{LoopStatus, Metadata, PlaybackStatus, Time, TrackId};

use crate::application::GatefoldApplication;
use crate::config::APP_ID;
use crate::covers;
use crate::player::Player;

/// Starts the MPRIS server (`org.mpris.MediaPlayer2.Gatefold`) on the main loop.
pub fn start(app: &GatefoldApplication, player: &Player) {
    let app = app.downgrade();
    let player = player.downgrade();
    glib::spawn_future_local(async move {
        // The development build has its own name, so both can run.
        let name = if APP_ID.ends_with(".Devel") {
            "Gatefold.Devel"
        } else {
            "Gatefold"
        };
        let server = match mpris_server::Player::builder(name)
            .identity("Gatefold")
            .desktop_entry(APP_ID)
            .can_raise(true)
            .can_quit(true)
            .can_play(true)
            .can_pause(true)
            .can_seek(true)
            .can_control(true)
            .build()
            .await
        {
            Ok(server) => server,
            Err(err) => {
                tracing::warn!("MPRIS is unavailable: {err}");
                return;
            }
        };
        let (Some(app), Some(player)) = (app.upgrade(), player.upgrade()) else {
            return;
        };
        connect_methods(&server, &app, &player);
        let server = std::rc::Rc::new(server);
        connect_updates(&server, &player);
        update_all(server.clone(), player.clone());
        server.run().await;
    });
}

fn connect_methods(server: &mpris_server::Player, app: &GatefoldApplication, player: &Player) {
    let weak = player.downgrade();
    let with = move |f: fn(&Player)| {
        let weak = weak.clone();
        move |_: &mpris_server::Player| {
            if let Some(player) = weak.upgrade() {
                f(&player);
            }
        }
    };
    server.connect_play_pause(with(Player::play_pause));
    server.connect_play(with(Player::play));
    server.connect_pause(with(Player::pause));
    server.connect_stop(with(Player::stop));
    server.connect_next(with(Player::next));
    server.connect_previous(with(Player::previous));

    let weak = player.downgrade();
    server.connect_seek(move |_, offset| {
        if let Some(player) = weak.upgrade() {
            let target = player.position() as i64 + offset.as_millis();
            player.seek(target.max(0) as u64);
        }
    });
    let weak = player.downgrade();
    server.connect_set_position(move |_, track_id, position| {
        if let Some(player) = weak.upgrade()
            && Some(track_id) == current_track_id(&player).as_ref()
            && position.as_millis() >= 0
            // The spec: positions past the end are ignored.
            && position.as_millis() as u64 <= player.duration()
        {
            player.seek(position.as_millis() as u64);
        }
    });
    let weak = player.downgrade();
    server.connect_set_shuffle(move |_, shuffle| {
        if let Some(player) = weak.upgrade() {
            player.set_shuffle(shuffle);
        }
    });
    let weak = player.downgrade();
    server.connect_set_loop_status(move |_, status| {
        if let Some(player) = weak.upgrade() {
            player.set_repeat(match status {
                LoopStatus::None => RepeatMode::Off,
                LoopStatus::Playlist => RepeatMode::All,
                LoopStatus::Track => RepeatMode::One,
            });
        }
    });
    let weak = player.downgrade();
    server.connect_set_volume(move |_, volume| {
        if let Some(player) = weak.upgrade() {
            player.set_volume(volume);
        }
    });

    let weak_app = app.downgrade();
    server.connect_raise(move |_| {
        if let Some(app) = weak_app.upgrade() {
            app.activate();
        }
    });
    let weak_app = app.downgrade();
    server.connect_quit(move |_| {
        if let Some(app) = weak_app.upgrade() {
            app.activate_action("quit", None);
        }
    });
}

fn connect_updates(server: &std::rc::Rc<mpris_server::Player>, player: &Player) {
    for property in ["current-track", "playing", "shuffle", "repeat", "duration"] {
        let server = server.clone();
        player.connect_notify_local(Some(property), move |player, _| {
            update_all(server.clone(), player.clone());
        });
    }
    // The volume lives in the settings, not in a property of the player.
    let server_ = server.clone();
    player
        .settings()
        .connect_changed(Some("volume"), move |settings, key| {
            let server = server_.clone();
            let volume = settings.double(key);
            glib::spawn_future_local(async move {
                if let Err(err) = server.set_volume(volume).await {
                    tracing::debug!("MPRIS volume update failed: {err}");
                }
            });
        });
    let server_ = server.clone();
    player.connect_notify_local(Some("position"), move |player, _| {
        server_.set_position(Time::from_millis(player.position() as i64));
    });
    let server_ = server.clone();
    player.connect_local("seeked", false, move |values| {
        let player = values[0].get::<Player>().ok()?;
        let server = server_.clone();
        let position = Time::from_millis(player.position() as i64);
        server.set_position(position);
        glib::spawn_future_local(async move {
            if let Err(err) = server.seeked(position).await {
                tracing::debug!("MPRIS seeked failed: {err}");
            }
        });
        None
    });
    let server_ = server.clone();
    player.connect_local("queue-changed", false, move |values| {
        let player = values[0].get::<Player>().ok()?;
        update_all(server_.clone(), player);
        None
    });
}

fn current_track_id(player: &Player) -> Option<TrackId> {
    let track = player.current_track()?;
    // Object paths take no "-": streamed songs (negative IDs) get an "s".
    let id = track.id();
    let part = if id < 0 {
        format!("s{}", id.unsigned_abs())
    } else {
        id.to_string()
    };
    TrackId::try_from(format!("/io/github/sobakah/Gatefold/Track/{part}")).ok()
}

fn metadata(player: &Player) -> Metadata {
    let Some(track) = player.current_track() else {
        return Metadata::builder().trackid(TrackId::NO_TRACK).build();
    };
    let record = &track.track().record;
    let mut builder = Metadata::builder()
        .title(track.title())
        .length(Time::from_millis(track.duration_ms() as i64));
    if let Some(id) = current_track_id(player) {
        builder = builder.trackid(id);
    }
    if let Some(artist) = track.artist() {
        builder = builder.artist([artist]);
    }
    if let Some(album) = &track.track().album_title {
        builder = builder.album(album.clone());
    }
    if let Some(album_artist) = track.track().album_artist.clone().filter(|a| !a.is_empty()) {
        builder = builder.album_artist([album_artist]);
    }
    if let Some(number) = record.tags.number {
        builder = builder.track_number(number as i32);
    }
    if let Some(uri) = track
        .cover_key()
        .and_then(|key| covers::uri(key, CoverSize::Player))
    {
        builder = builder.art_url(uri);
    }
    if let Some(uri) = gatefold_core::playback::file_uri(&record.path) {
        builder = builder.url(uri);
    }
    builder.build()
}

fn update_all(server: std::rc::Rc<mpris_server::Player>, player: Player) {
    glib::spawn_future_local(async move {
        let status = match (player.playing(), player.current_track().is_some()) {
            (true, _) => PlaybackStatus::Playing,
            (false, true) => PlaybackStatus::Paused,
            (false, false) => PlaybackStatus::Stopped,
        };
        let loop_status = match RepeatMode::from_str_lossy(&player.repeat()) {
            RepeatMode::Off => LoopStatus::None,
            RepeatMode::All => LoopStatus::Playlist,
            RepeatMode::One => LoopStatus::Track,
        };
        server.set_position(Time::from_millis(player.position() as i64));
        let results = [
            server.set_metadata(metadata(&player)).await,
            server.set_playback_status(status).await,
            server.set_loop_status(loop_status).await,
            server.set_shuffle(player.shuffle()).await,
            server.set_can_go_next(player.has_next()).await,
            server.set_can_go_previous(player.has_previous()).await,
            server.set_volume(player.volume()).await,
        ];
        for result in results {
            if let Err(err) = result {
                tracing::debug!("MPRIS update failed: {err}");
            }
        }
    });
}
