<p align="center"><img src="data/icons/hicolor/scalable/apps/io.github.sobakah.Jewelcase.svg" width="128" alt=""></p>

# Jewelcase

A music player for GNOME, for people who keep their own music as albums and use Spotify as well.

Jewelcase is built around your local library: covers to browse, albums and artists that are tidy, lyrics, gapless playback. Spotify fills the gaps. Its playlists, saved albums and artists sit next to your own, and the songs you don't have play through Spotify in the same queue. Without Spotify, Jewelcase is a complete local player.

## Your library and Spotify together

- **What you own plays from your files**, also when it's part of a Spotify playlist: gapless, with ReplayGain and the lyrics in your files. Jewelcase matches Spotify songs to your files by ISRC, title, artist and length.
- **What you don't own comes from Spotify**: in playlists, in search results (after your library's) and on artist pages ("On Spotify": the releases you don't have). It streams through Spotify Soloist, Spotify's official player, in Jewelcase's queue.
- **Playlists go both ways.** Your Spotify playlists show next to your own; edit them from here (songs, order, name, cover). Publish a local playlist to Spotify. Add songs from your library or from Spotify to any playlist.
- **Saved albums and followed artists** that aren't in your library get their own section in the album and artist grids.
- **Play on your Spotify Connect devices**, with the player showing what they play.

## Features

- **Albums**: a grid of covers. Editions (deluxe, remastered, a Japanese version) are merged into one album. An album opens beside the grid, and each song unfolds to show its details.
- **Artists**: album artists only, so featured guests don't clutter the list. Artist pages group albums, EPs & singles, soundtracks and collaborations, and list the artist's songs on other albums. Portraits, a short bio and links come from online sources if you allow it.
- **Player**: a cover-coloured full player with synced lyrics. Korean, Japanese and Chinese lyrics show with a romanization. Crossfade follows where songs really fade, and albums stay gapless. Media keys and GNOME's media controls work.
- **Queue and smart shuffle**: *Play next*, *Add to queue*, a reorderable queue that's kept between starts. Shuffle songs or albums, or mix in similar songs by genre, tempo, artist or sound.
- **Playlists and search**: mosaic covers, drag and drop, search within a playlist, import and export (M3U8, XSPF). One search (Ctrl+F) finds artists, albums, songs and playlists.
- **Your files stay yours**: Jewelcase never writes to your music files. Separate tools do, and Jewelcase picks up their changes:
  - [lyrembed](https://github.com/sobakah/lyrembed) finds and edits lyrics and romanizes them;
  - [cdport](https://github.com/sobakah/cdport) imports CDs and edits tags;
  - [Ear Tag](https://flathub.org/apps/app.drey.EarTag), Kid3 or Picard can edit tags instead.

  Preferences → Integrations shows the steps to install and set them up.

## Install

Jewelcase is in pre-release, for Fedora 45. Download the RPM from the [releases page](https://github.com/sobakah/jewelcase/releases) and install it:

```sh
sudo dnf install ./jewelcase-*.x86_64.rpm
```

Jewelcase tells you when a new version is out and installs it for you.

Your library is kept in `~/.local/share/io.github.sobakah.Jewelcase/`, and cover thumbnails in `~/.cache/io.github.sobakah.Jewelcase/`.

## Connecting Spotify

You need Spotify Premium and a Spotify developer app of your own. Preferences → Spotify walks you through it:

1. Create an app at <https://developer.spotify.com/dashboard> (any name) and choose "Web API".
2. Add the redirect URI `http://127.0.0.1:8974/callback`, exactly as written.
3. Paste the app's client ID into Preferences and press *Connect*.

The login is kept in your keyring.

**To stream songs you don't own**, install [Spotify Soloist](https://developer.spotify.com/documentation/soloist/reference/downloads-and-updates) and get a free Soloist API key. Enter the key under Preferences → Spotify → Streaming. You can also let [soloistd](https://github.com/MopigamesYT/soloistd) install Soloist, keep it updated and run it in the background; Jewelcase then uses it, and Preferences can control it.

The first time, link Soloist by choosing it in the Spotify app's device list.

## Development

On Fedora 45:

```sh
sudo dnf install cargo rust rustfmt clippy cargo-rpm-macros \
    meson blueprint-compiler gettext desktop-file-utils appstream \
    gtk4-devel libadwaita-devel \
    gstreamer1-devel gstreamer1-plugins-base-devel sqlite-devel \
    fontconfig-devel libseccomp-devel \
    gstreamer1-plugins-good gstreamer1-plugin-libav glycin-loaders bubblewrap \
    mock rpm-build
```

| Command | What it does |
|---|---|
| `make run` | Builds and runs the development version |
| `make test` | Unit tests, and checks the desktop file and metainfo |
| `make lint` | `cargo fmt --check` and `cargo clippy` |
| `make rpm` | Builds the RPM on this machine (`rpmbuild/RPMS/`) |
| `make mock` | Builds the RPM in a clean Fedora 45 chroot (needs the `mock` group) |
| `make flatpak` | Builds and installs a test Flatpak ([build-aux/flatpak](build-aux/flatpak/README.md)) |

The development version has its own app ID (`io.github.sobakah.Jewelcase.Devel`), so it never touches the installed app's library, settings or logins.

Meson builds the UI resources (Blueprint, CSS, icons) and runs Cargo, which embeds them in the program. `cargo check` and `cargo clippy` work on their own, but a program built only by Cargo won't start.

**Testing without disturbing your desktop.** `build-aux/headless.sh COMMAND` runs a command in a private, invisible session. That session has a headless compositor, its own D-Bus, in-memory settings, silent audio and its own data in `_build/headless/`. For example, this renders the window to a PNG:

```sh
GSETTINGS_SCHEMA_DIR=_build/data JEWELCASE_SCREENSHOT=/tmp/shot.png \
    build-aux/headless.sh _build/app/jewelcase
```

`setup_screenshot` in `app/src/window.rs` lists the variables that open albums, playlists or Preferences, play songs or run actions first. Debug builds talk to a stand-in server instead of Spotify when `JEWELCASE_SPOTIFY_TEST_SERVER=http://127.0.0.1:<port>` is set. `JEWELCASE_LOG=debug` (or `trace`, `warn`) sets the log level.

**Layout:**

```
app/        GTK interface (crate `jewelcase`)
core/       library, scanner, queue, playback, similarity, matching; no GTK
lyrics/     embedded and sidecar lyrics, LRC, romanization pairing
metadata/   MusicBrainz, Wikidata, Commons and Deezer lookups
spotify/    Spotify login (OAuth PKCE) and Web API
soloist/    Spotify Soloist over its local WebSocket API, and soloistd
stream/     optional librespot streaming (feature `spotify-stream`)
tools/      lyrembed, cdport and tag editors
data/       desktop file, metainfo, settings schema, icons, UI (data/resources)
po/         translations
build-aux/  headless test session, Flatpak manifest, build helpers
```

[CONCEPT.md](CONCEPT.md) describes the design and the reasons behind it, and [CHANGELOG.md](CHANGELOG.md) lists what each release changed.

## License

Jewelcase's code is GPL-3.0-or-later. It uses bliss-audio for smart shuffle, which is GPL-3.0-only, so the program as built is GPL-3.0-only.
