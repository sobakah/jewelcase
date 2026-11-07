# Gatefold

A local-first music player for GNOME, built for people who listen to **albums**, follow **artists** and curate **playlists**. The design and roadmap are in [CONCEPT.md](CONCEPT.md).

**Status:** Phase 0 (skeleton). The window, the bottom-sheet player, preferences with music-folder selection, and the build and packaging setup are in place. The app doesn't scan or play music yet.

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

The account needs Spotify Premium. The login is stored in the keyring. Then:

- **Play On…** in the player lists your Spotify Connect devices; the player then controls the
  device and shows what it plays (with lyrics and cover from your library when it has the song).
- **Spotify playlists** appear next to local ones; entries that are in your library play from
  your files (ISRC, then title, artist and length). Your own playlists can be edited here
  and the changes are sent to Spotify; *Create on Spotify* publishes a local playlist.

**Streaming** (optional, unofficial): with the `spotify-stream` feature (`meson -Dspotify_stream=true`,
on in `make run` builds), Preferences → Spotify → Streaming plays songs that aren't in your
library through librespot. It needs a second, one-time browser login. It's against Spotify's
terms of use and may stop working at any time; your own files always play from the library.

For development, debug builds talk to a stand-in server instead of Spotify when
`GATEFOLD_SPOTIFY_TEST_SERVER=http://127.0.0.1:<port>` is set.

## Layout

```
app/        GTK UI (crate `gatefold`)
core/       library model, scanner, queue, playback; no GTK (crate `gatefold-core`)
tools/      lyrembed, cdimport, tag editor: detection, commands, configs (crate `gatefold-tools`)
lyrics/     LRC parsing, embedded and sidecar lyrics, dual-line pairing (crate `gatefold-lyrics`)
metadata/   MusicBrainz, Wikidata, Commons, fanart.tv lookups (crate `gatefold-metadata`)
spotify/    Spotify login (OAuth PKCE) and Web API: player, playlists, search (crate `gatefold-spotify`)
data/       desktop file, metainfo, GSettings schema, icons, Blueprint UI + CSS (data/resources)
po/         translations
build-aux/  build helper scripts
```

Similarity (CONCEPT.md §7) lives in `core` (`similar.rs`, `analysis.rs`), as does matching
streaming tracks to the library (`matching.rs`).

## License

GPL-3.0-or-later
