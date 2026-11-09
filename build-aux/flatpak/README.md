# Test Flatpak

A Flatpak of the working tree, for trying Gatefold the way it would run as a
Flatpak (CONCEPT.md §13, phase 9). It isn't the release Flatpak yet.

## Build and run

Needs `flatpak-builder` (or the Flathub app `org.flatpak.Builder`) and
Flathub as a remote of the user installation:

```sh
flatpak remote-add --user --if-not-exists flathub https://dl.flathub.org/repo/flathub.flatpakrepo
flatpak install --user flathub org.flatpak.Builder   # if flatpak-builder isn't installed
make flatpak                                         # build + install for this user
flatpak run io.github.sobakah.Gatefold.Devel
```

`make flatpak` installs the GNOME 51 SDK and the Rust extension when they're
missing. The first build compiles everything (several minutes); the build
files are in `_build/flatpak`.

`make flatpak-bundle` also writes `_build/flatpak/gatefold-devel.flatpak`, a
single file for another computer (`flatpak install --user gatefold-devel.flatpak`;
it fetches the GNOME runtime from Flathub). A bundle doesn't update itself.

Remove it with `flatpak uninstall --user io.github.sobakah.Gatefold.Devel`
(add `--delete-data` to also remove `~/.var/app/io.github.sobakah.Gatefold.Devel`).

## What's different from the RPM

- **Its own app ID** (`.Devel`, with the striped header bar of development
  builds), so it runs next to the installed RPM. Its library database,
  settings, covers and Spotify login are its own (in
  `~/.var/app/io.github.sobakah.Gatefold.Devel`); it scans the library anew.
- **The music folder is read-only** (`--filesystem=xdg-music:ro`). Folders
  outside it would come through the folder chooser's portal (untested).
- **Spotify logins** are kept through the secret portal; on desktops without
  one, in a file only the user can read (Preferences › Spotify says so).
- **Doesn't work yet** (they need `flatpak-spawn --host`, phase 9): lyrembed
  (lyrics search, romanization), cdimport, Ear Tag (`flatpak run` from inside
  the sandbox) and soloistd's controls. Gatefold shows them as not installed.
  Soloist, which Gatefold downloads and runs itself, should work inside the
  sandbox (untested).
- **No update check**: Flatpaks update through their repository.

## Tested (2026-11-09, headless)

Scanning the library (read-only), covers (glycin's loaders from the runtime),
the album panel, playback and the visualizer, and the `bpmdetect` element for
the song analysis. For an invisible, silent run like `build-aux/headless.sh`:

```sh
FLATPAK_USER_DIR=$HOME/.local/share/flatpak build-aux/headless.sh \
  flatpak run --env=GSETTINGS_BACKEND=memory --env=GATEFOLD_SILENT=1 \
  --env=GATEFOLD_NO_KEYRING=1 --env=GATEFOLD_NO_BROWSER=1 --env=GDK_DEBUG=no-portals \
  --filesystem=/tmp --env=GATEFOLD_SCREENSHOT=/tmp/shot.png io.github.sobakah.Gatefold.Devel
```

Flatpak keeps the sandbox's own `XDG_DATA_HOME`, so such a run writes the
library database to `~/.var/app/io.github.sobakah.Gatefold.Devel`.

## Not for Flathub as it is

Cargo downloads the crates while building (`--share=network` for the build).
Flathub builds offline: the crates would come from a `cargo-sources.json`
generated with flatpak-builder-tools' `flatpak-cargo-generator.py`. Flathub
also needs the release app ID, a public repository and the other points in
CONCEPT.md §13.
