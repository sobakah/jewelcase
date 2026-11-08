//! A song's details as label/value rows: in expanded song rows, and (with
//! everything) in the player's Song Info card.

use gettextrs::gettext;
use gtk::prelude::*;

use crate::application::GatefoldApplication;
use crate::format;
use crate::objects::TrackObject;

/// `full` adds album, numbering, release and file details.
pub fn grid(track: &TrackObject, full: bool) -> gtk::Grid {
    let tags = &track.track().record.tags;
    let grid = gtk::Grid::builder()
        .column_spacing(12)
        .row_spacing(3)
        .build();
    let mut row = 0;
    let mut add = |key: String, value: String| {
        grid.attach(
            &gtk::Label::builder()
                .label(key)
                .xalign(1.0)
                .yalign(0.0)
                .css_classes(["dim-label", "caption"])
                .build(),
            0,
            row,
            1,
            1,
        );
        grid.attach(
            &gtk::Label::builder()
                .label(value)
                .xalign(0.0)
                .wrap(true)
                .wrap_mode(gtk::pango::WrapMode::WordChar)
                .selectable(true)
                .hexpand(true)
                .max_width_chars(40)
                .css_classes(["caption"])
                .build(),
            1,
            row,
            1,
            1,
        );
        row += 1;
    };
    if let Some(artist) = &tags.artist {
        add(gettext("Artist"), artist.clone());
    }
    if full {
        if let Some(album) = &tags.album {
            add(gettext("Album"), album.clone());
        }
        if let Some(album_artist) = &tags.album_artist {
            add(gettext("Album Artist"), album_artist.clone());
        }
        let numbering = match (tags.disc, tags.number) {
            // Translators: disc and track number, e.g. "Disc 1, Track 4".
            (Some(disc), Some(number)) => Some(
                gettext("Disc {disc}, Track {track}")
                    .replace("{disc}", &disc.to_string())
                    .replace("{track}", &number.to_string()),
            ),
            (None, Some(number)) => {
                Some(gettext("Track {track}").replace("{track}", &number.to_string()))
            }
            _ => None,
        };
        if let Some(numbering) = numbering {
            add(gettext("Number"), numbering);
        }
        if let Some(year) = tags.year {
            add(gettext("Released"), year.to_string());
        }
        if let Some(kind) = &tags.release_type {
            add(gettext("Release Type"), kind.clone());
        }
    }
    if let Some(composer) = &tags.composer {
        add(gettext("Composer"), composer.clone());
    }
    if !tags.genres.is_empty() {
        add(gettext("Genre"), tags.genres.join(", "));
    }
    if let Some(bpm) = tags.bpm {
        add(gettext("BPM"), format!("{bpm:.0}"));
    }
    add(gettext("Format"), format::audio_format(tags));
    if full {
        if let Some(channels) = tags.channels {
            let value = match channels {
                1 => gettext("Mono"),
                2 => gettext("Stereo"),
                n => n.to_string(),
            };
            add(gettext("Channels"), value);
        }
        if let Some(gain) = tags.replaygain_track {
            let mut value = format!("{gain:+.2} dB");
            if let Some(album) = tags.replaygain_album {
                // Translators: ReplayGain, e.g. "-6.20 dB (album -7.10 dB)".
                value.push_str(&gettext(" (album {} dB)").replace("{}", &format!("{album:+.2}")));
            }
            add(gettext("ReplayGain"), value);
        }
        if let Some(isrc) = &tags.isrc {
            add(gettext("ISRC"), isrc.clone());
        }
        if let Some(barcode) = &tags.barcode {
            add(gettext("Barcode"), barcode.clone());
        }
    }
    // The playlists that have it, local or Spotify ones.
    let library = GatefoldApplication::get().library();
    let mut playlists: Vec<String> = library
        .playlists_containing(track)
        .into_iter()
        .filter_map(|id| library.playlist(id))
        .map(|p| p.playlist().name.clone())
        .collect();
    playlists.sort_by_key(|name| name.to_lowercase());
    add(
        gettext("Playlists"),
        if playlists.is_empty() {
            gettext("None")
        } else {
            playlists.join(", ")
        },
    );
    // What kind of lyrics there are, not the text.
    let path = &track.track().record.path;
    let mut lyrics = Vec::new();
    if let Some(embedded) = &tags.lyrics {
        lyrics.push(if embedded.synced {
            gettext("Synced, embedded")
        } else {
            gettext("Plain, embedded")
        });
    }
    if path.with_extension("lrc").is_file() {
        lyrics.push(gettext("Synced, separate file (.lrc)"));
    } else if path.with_extension("txt").is_file() {
        lyrics.push(gettext("Plain, separate file (.txt)"));
    }
    add(
        gettext("Lyrics"),
        if lyrics.is_empty() {
            gettext("None")
        } else {
            lyrics.join("; ")
        },
    );
    if full {
        add(
            gettext("File"),
            track.track().record.path.display().to_string(),
        );
    }
    grid
}
