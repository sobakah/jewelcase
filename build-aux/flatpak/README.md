# Test Flatpak

A Flatpak of the working tree, for trying Jewelcase the way it would run as a
Flatpak (CONCEPT.md §13, phase 9). It isn't the release Flatpak yet.

## Build and run

Needs `flatpak-builder` (or the Flathub app `org.flatpak.Builder`) and
Flathub as a remote of the user installation:

```sh
flatpak remote-add --user --if-not-exists flathub https://dl.flathub.org/repo/flathub.flatpakrepo
flatpak install --user flathub org.flatpak.Builder   # if flatpak-builder isn't installed
make flatpak                                         # build + install for this user
flatpak run io.github.sobakah.Jewelcase.Devel
```

`make flatpak` installs the GNOME 51 SDK and the Rust extension when they're
missing. The first build compiles everything (several minutes); the build
files are in `_build/flatpak`.

`make flatpak-bundle` also writes `_build/flatpak/jewelcase-devel.flatpak`, a
single file for another computer (`flatpak install --user jewelcase-devel.flatpak`;
it fetches the GNOME runtime from Flathub). A bundle doesn't update itself.

Remove it with `flatpak uninstall --user io.github.sobakah.Jewelcase.Devel`
(add `--delete-data` to also remove `~/.var/app/io.github.sobakah.Jewelcase.Devel`).

## What's different from the RPM

- **Its own app ID** (`.Devel`, with the striped header bar of development
  builds), so it runs next to the installed RPM. Its library database,
  settings, covers and Spotify login are its own (in
  `~/.var/app/io.github.sobakah.Jewelcase.Devel`); it scans the library anew.
- **The music folder is read-only** (`--filesystem=xdg-music:ro`). Folders
  outside it would come through the folder chooser's portal (untested).
- **Spotify logins** are kept through the secret portal; on desktops without
  one, in a file only the user can read (Preferences › Spotify says so).
- **cdport and lyrembed** work through their D-Bus services on the host
  (`cdport --install-dbus-service`, `lyrembed --install-dbus-service`;
  `--talk-name=io.github.sobakah.cdport`, `…lyrembed`). Preferences ›
  Integrations shows the steps when they aren't set up.
- **Soloist** runs inside the sandbox as the Flatpak's own: the binary
  soloistd downloads and updates on the host (`~/.local/share/soloistd/bin`,
  read-only), with the Soloist API key entered in the Flatpak's Preferences
  (the same one soloistd uses: Spotify gives one per account), its own
  data folder (linked once in the Spotify app) and the device name
  "Jewelcase Devel". soloistd manages only one Soloist, so it stays with the
  installed app; the Flatpak doesn't show its controls.
- **Doesn't work yet**: Ear Tag (`flatpak run` from inside the sandbox).
  Jewelcase shows it as not installed.
- **No update check**: Flatpaks update through their repository.

## Tested (2026-11-09, headless)

Scanning the library (read-only), covers (glycin's loaders from the runtime),
the album panel, playback and the visualizer, and the `bpmdetect` element for
the song analysis. cdport over D-Bus (with a test service file in the
headless session's `XDG_DATA_HOME/dbus-1/services`): its status, unfinished
imports, an import's page address, Stop and the `Finished` signal. For an invisible, silent run like `build-aux/headless.sh`:

```sh
FLATPAK_USER_DIR=$HOME/.local/share/flatpak build-aux/headless.sh \
  flatpak run --env=GSETTINGS_BACKEND=memory --env=JEWELCASE_SILENT=1 \
  --env=JEWELCASE_NO_KEYRING=1 --env=JEWELCASE_NO_BROWSER=1 --env=GDK_DEBUG=no-portals \
  --filesystem=/tmp --env=JEWELCASE_SCREENSHOT=/tmp/shot.png io.github.sobakah.Jewelcase.Devel
```

Flatpak keeps the sandbox's own `XDG_DATA_HOME`, so such a run writes the
library database to `~/.var/app/io.github.sobakah.Jewelcase.Devel`.

## Not for Flathub as it is

Cargo downloads the crates while building (`--share=network` for the build).
Flathub builds offline: the crates would come from a `cargo-sources.json`
generated with flatpak-builder-tools' `flatpak-cargo-generator.py`. Flathub
also needs the release app ID, a public repository and the other points in
CONCEPT.md §13.
