# Gatefold

A local-first music player for GNOME, built for people who listen to **albums**, follow **artists** and curate **playlists**. The design and roadmap are in [CONCEPT.md](CONCEPT.md).

**Status:** Phase 0 (skeleton). The window, the bottom-sheet player, preferences with music-folder selection, and the build and packaging setup are in place. The app doesn't scan or play music yet.

## Development setup (Fedora 45)

```sh
sudo dnf install cargo rust rustfmt clippy cargo-rpm-macros \
    meson blueprint-compiler gettext desktop-file-utils appstream \
    gtk4-devel libadwaita-devel \
    gstreamer1-devel gstreamer1-plugins-base-devel sqlite-devel \
    mock rpm-build
sudo usermod -aG mock "$USER"   # for `make mock`; log in again afterwards
```

The GStreamer and SQLite headers aren't needed until Phase 1.

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

## Layout

```
app/        GTK UI (crate `gatefold`)
core/       library model, scanner, queue, playback; no GTK (crate `gatefold-core`)
data/       desktop file, metainfo, GSettings schema, icons, Blueprint UI + CSS (data/resources)
po/         translations
build-aux/  build helper scripts
```

The `metadata`, `lyrics`, `tools`, `similarity` and `spotify` crates from CONCEPT.md §2 are added in the phases that need them.

## License

GPL-3.0-or-later
