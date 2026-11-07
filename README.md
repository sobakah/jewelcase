# Gatefold

A local-first music player for GNOME, built for people who listen to **albums**, follow **artists** and curate **playlists**. Cover art is the main way around your library; lyrics, the queue and details open when you want them. Spotify is optional and stays in its own sections.

**Status:** 0.1.0 is the first pre-release, for Fedora 45. What changed in each version is in [CHANGELOG.md](CHANGELOG.md); the design and roadmap are in [CONCEPT.md](CONCEPT.md).

## Features

**Albums.** Gatefold scans your music folders and groups the songs into albums. Editions of the same album (deluxe, remastered, a Japanese version) are merged into one, with the edition as a subtitle. The album grid shows only covers, titles and artists, and fills the window at any width; sort it by artist, title, year or date added. Clicking an album opens its songs in a side panel, where each song unfolds to show its details. Changes to your files show up live, and folders that are temporarily missing (an unmounted drive, a NAS that's off) keep their songs.

**Artists.** The artist list holds album artists only, so featured guests don't clutter it. An artist page groups the albums into albums, EPs & singles, soundtracks and collaborations, and lists under "Appears on" the artist's songs on other artists' albums. With online lookups turned on (you're asked at first start), Gatefold fetches portraits from Spotify (when connected), Deezer and Wikimedia Commons, a short Wikipedia bio, and links to Wikipedia, MusicBrainz and Spotify. Portraits can be replaced with your own image or another online one.

**Player.** The mini player at the bottom slides up into a full player with the cover as large as possible, coloured by the cover. Lyrics and the queue open next to it on wide windows. Synced lyrics come from the files or `.lrc` files and follow the song; for Korean, Japanese and Chinese songs an original line and its romanization are shown together, and missing romanization is added automatically through lyrembed. Playback is gapless, with ReplayGain and loudness levelling for songs without ReplayGain tags. Crossfade (up to 12 s) follows where songs actually fade out and start, and albums played in order stay gapless. Media keys and GNOME's media controls work through MPRIS.

**Queue and shuffle.** "Play next" and "Add to queue" go before the rest of the album or playlist. The queue can be reordered (also with Alt+↑/↓), cleared and saved as a playlist, and it's restored when Gatefold starts. Shuffle songs or whole albums, or use **smart shuffle**: mix in similar songs by genre, tempo, artist or sound, and keep playing similar music when the queue ends. The song analysis behind it runs in the background once turned on in Preferences.

**Playlists and search.** Playlists get mosaic covers from their albums, can be reordered by drag and drop, and can be imported and exported as M3U8 or XSPF. One search (Ctrl+F) finds artists, albums, songs and playlists, in a card over the current view.

**Companion tools.** Gatefold never writes to your music files itself; it starts these separately installed tools and picks up their changes:

- **lyrembed** finds lyrics automatically, or opens its page to check and edit them.
- **cdimport** imports CDs; a banner offers it when you insert one.
- [Ear Tag](https://flathub.org/apps/app.drey.EarTag) (or Kid3, Picard, or a command of your own) edits tags and covers.

**Spotify (optional).** With Spotify Premium and a developer app of your own (see [Spotify](#spotify-optional) below):

- Play on your Spotify Connect devices, with the player showing what they play.
- Your Spotify playlists appear next to the local ones. Songs you have play from your files, your own playlists can be edited from Gatefold, and local playlists can be published to Spotify.
- Saved albums and followed artists that aren't in your library get their own section at the end of the album and artist grids. Artist pages list the artist's releases on Spotify that you don't have.
- Songs you don't have stream through Spotify Soloist, Spotify's official headless player, in Gatefold's queue.

## Install

Fedora 45: download the RPM from the [releases page](https://github.com/sobakah/gatefold/releases) and install it with

```sh
sudo dnf install ./gatefold-0.1.0-1.fc45.x86_64.rpm
```

To build the RPM yourself, install the build dependencies (`sudo dnf builddep gatefold.spec`) and run `make rpm`; the packages end up in `rpmbuild/RPMS/`.

Your library lives in `~/.local/share/io.github.sobakah.Gatefold/`, cover thumbnails in `~/.cache/io.github.sobakah.Gatefold/`.

## Development setup (Fedora 45)

```sh
sudo dnf install cargo rust rustfmt clippy cargo-rpm-macros \
    meson blueprint-compiler gettext desktop-file-utils appstream \
    gtk4-devel libadwaita-devel \
    gstreamer1-devel gstreamer1-plugins-base-devel sqlite-devel \
    fontconfig-devel libseccomp-devel \
    gstreamer1-plugins-good gstreamer1-plugin-libav glycin-loaders bubblewrap \
    mock rpm-build
sudo usermod -aG mock "$USER"   # for `make mock`; log in again afterwards
```

The library database lives in `~/.local/share/<app id>/library.sqlite`,
cover thumbnails in `~/.cache/<app id>/covers/` (the development build uses
the `.Devel` app ID, so it never touches the installed app's data).

Development aids:

- `build-aux/headless.sh COMMAND…` runs a command in a private, invisible
  session: headless mutter, its own D-Bus bus, in-memory GSettings, silent
  audio and separate data folders (`_build/headless/`). Use it for any UI or
  playback test that shouldn't show up on the desktop, e.g.
  `GSETTINGS_SCHEMA_DIR=_build/data GATEFOLD_SCREENSHOT=/tmp/shot.png build-aux/headless.sh _build/app/gatefold`.
- `GATEFOLD_SILENT=1` plays into a fakesink instead of the speakers.
- `GATEFOLD_NO_BROWSER=1` logs companion tool pages (lyrembed, cdimport)
  instead of opening them in the browser.
- `GATEFOLD_SCREENSHOT=out.png` (development profile only) renders the
  window to a PNG and quits;
  see `setup_screenshot` in `app/src/window.rs` for the other variables.
- `cargo run -p gatefold-core --example scan -- DB COVERS FOLDER…` scans
  into a throwaway database and lists the albums.
- `cargo run -p gatefold-core --example analyze -- FILE…` analyzes songs
  like the smart shuffle worker (fingerprint, bliss features, BPM).

## Build and run

| Command | What it does |
|---|---|
| `make run` | Development build (app ID `io.github.sobakah.Gatefold.Devel`), run from the build directory |
| `make test` | Rust unit tests, plus validation of the desktop file and metainfo |
| `make lint` | `cargo fmt --check` and `cargo clippy` |
| `make rpm` | RPM built on this machine (`rpmbuild`) |
| `make mock` | RPM built in a clean Fedora 45 chroot |

Set `GATEFOLD_LOG=debug` (or `trace`, `warn`, …) to change the log level.

Meson runs Cargo and passes it the compiled GResource bundle (Blueprint UI, CSS, icons), which is embedded in the binary. `cargo check` and `cargo clippy` work without Meson, but a binary built only by Cargo won't start.

## Spotify (optional)

Gatefold connects to Spotify with a client ID of your own (Spotify no longer gives apps
like this a shared one). Preferences → Spotify walks through it:

1. Create an app at <https://developer.spotify.com/dashboard> (any name), choose “Web API”.
2. Add the redirect URI `http://127.0.0.1:8974/callback` exactly.
3. Paste the client ID into Preferences and press Connect.

The account needs Spotify Premium. The login is stored in the keyring. What Gatefold then does with Spotify is listed under [Features](#features).

**Streaming** (optional): Preferences → Spotify → Streaming plays songs that aren't in your
library, in Gatefold's queue. Your own files always play from the library.

- **Spotify Soloist** (default): Spotify's official headless client. Install it from Spotify's
  [downloads page](https://developer.spotify.com/documentation/soloist/reference/downloads-and-updates),
  or let [soloistd](https://github.com/MopigamesYT/soloistd) install and update it; Gatefold then
  uses it and needs nothing else. Preferences → Spotify → soloistd controls the service (start,
  stop, restart), its settings (Gatefold's access, device name, PipeWire output, volume at start,
  API key), updates Soloist, links it again and shows its log. Without
  soloistd, Gatefold starts Soloist itself with your Soloist API key. Link it once by choosing it
  in the Spotify app's device list. Builds expire after 90 days. Spotify's terms don't clearly
  cover controlling it from another app.
- **librespot** (unofficial): with the `spotify-stream` feature (`meson -Dspotify_stream=true`, on
  in `make run` builds). Needs a second browser login, is against Spotify's terms of use, and
  doesn't work on some accounts (Spotify refuses its audio keys).

For development, debug builds talk to a stand-in server instead of Spotify when
`GATEFOLD_SPOTIFY_TEST_SERVER=http://127.0.0.1:<port>` is set.

## Layout

```
app/        GTK UI (crate `gatefold`)
core/       library model, scanner, queue, playback; no GTK (crate `gatefold-core`)
tools/      lyrembed, cdimport, tag editor: detection, commands, configs (crate `gatefold-tools`)
lyrics/     LRC parsing, embedded and sidecar lyrics, dual-line pairing (crate `gatefold-lyrics`)
metadata/   MusicBrainz, Wikidata, Commons, Deezer lookups (crate `gatefold-metadata`)
spotify/    Spotify login (OAuth PKCE) and Web API: player, playlists, search (crate `gatefold-spotify`)
soloist/    Spotify Soloist control over its local WebSocket API (crate `gatefold-soloist`)
stream/     optional librespot streaming into GStreamer (crate `gatefold-stream`, feature `spotify-stream`)
data/       desktop file, metainfo, GSettings schema, icons, Blueprint UI + CSS (data/resources)
po/         translations
build-aux/  build helper scripts
```

Similarity (CONCEPT.md §7) lives in `core` (`similar.rs`, `analysis.rs`), as does matching
streaming tracks to the library (`matching.rs`).

## License

Gatefold's code is GPL-3.0-or-later. It uses bliss-audio (smart shuffle),
which is GPL-3.0-only, so the program as built is GPL-3.0-only.
