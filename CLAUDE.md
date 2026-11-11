# Jewelcase – notes for Claude Code

Jewelcase is a GNOME music player for a local album library with Spotify woven in: songs you own
play from your files (also inside Spotify playlists), songs you don't stream through Spotify
Soloist in the same queue. Rust (edition 2024, rust ≥ 1.95), gtk4-rs 0.11, libadwaita 0.9
(feature `v1_10`), GStreamer 1.28, SQLite, Blueprint UI, built by Meson driving Cargo; packaged
as an RPM for Fedora 45. User docs: [README.md](README.md); the design and its reasons:
[CONCEPT.md](CONCEPT.md); the test Flatpak: [build-aux/flatpak/README.md](build-aux/flatpak/README.md).

## The program family

<!-- This section is the same in all six repositories. Change it in all of them or none. -->

Six programs by the same owner, each in its own private repository `sobakah/<name>` with its own
CLAUDE.md. Other sessions work in the other repositories: change one only when the owner asks.

| Program | What it does | Works with |
|---|---|---|
| picktag | Tags and searches pictures, videos and documents (GTK 4, libadwaita) | reads keepbest's and media-optimizer's file journals; starts keepbest for *Find Duplicates* |
| keepbest | Finds duplicate pictures and videos and keeps the best copy (page in the browser) | hands its tab to media-optimizer; writes a file journal |
| media-optimizer | Converts pictures, GIFs and videos to smaller formats (page in the browser) | started from keepbest; writes a file journal |
| cdport | Rips CDs, tags them and adds covers and lyrics (page in the browser) | starts lyrembed; D-Bus service for Jewelcase |
| lyrembed | Finds lyrics, embeds them in music files, romanizes them (page in the browser) | started from cdport; D-Bus service for Jewelcase |
| Jewelcase | A GNOME music player for an album library, with Spotify (GTK 4, libadwaita) | uses cdport and lyrembed; never writes music files |

The file journal format is specified in picktag's `docs/JOURNAL_SPEC.md`. The programs with a page
share its look (libadwaita's design language): `static/base.css` and `static/icons.js` are
lyrembed's, copied unchanged into cdport, keepbest and media-optimizer; change them in lyrembed and
copy them back. Setup details of the owner's machine are in `CLAUDE.local.md` (not in git).

### Working rules

- **Commits**: commit finished work once its tests pass, locally, on the default branch;
  experiments stay uncommitted. **Push, tags and releases only when the owner says so**; a request
  to release includes commit, tag and push. Pushed history is never rewritten; a mistake in a
  release is fixed with a new version.
- **Commit messages**: English; a subject of at most 72 characters saying what changed; a body
  wrapped at 72 saying why and what was verified. Written directly, no draft files.
- **Before pushing**, check the diff for personal data: the owner's name, home paths, file and
  folder names or contents from the owner's libraries, details of the owner's machine. Those
  belong in `CLAUDE.local.md`.
- **Verify, don't assert.** Run the code before saying something works; say what was tested and
  what wasn't. Outside reviews have been wrong before: check each claim.
- **Every bug fix comes with a test that fails without it.**
- **Tests never touch the owner's data** (libraries, music, settings, journals, the trash), the
  network or real hardware; they use temporary folders and generated media.
- **Ask first** before installing system packages, deleting or overwriting the owner's files, or
  changing a default the owner chose.
- Stop processes by PID, never with `pkill -f`. Commands for the owner to paste are written for
  fish; examples in the docs use bash.

### Versions and releases

- Versions have three parts and tags no `v` (`1.2.0`). New features or changed behaviour raise the
  minor version, fixes alone the patch. The version lives in one place (see Releasing below).
- Every user-visible change goes under `## [Unreleased]` at the top of `CHANGELOG.md` when it is
  committed. Don't propose a release after every change: say what is waiting and suggest one when
  a feature or a few fixes have gathered.
- A release: the version raised, `## [Unreleased]` becomes `## [X.Y.Z] — YYYY-MM-DD`, the full test
  suite passes, one commit `<program> X.Y.Z: <what it brings>`, an annotated tag
  `git tag -a X.Y.Z -m "<program> X.Y.Z"`, push of the branch and the tag, then a GitHub release
  titled `<program> X.Y.Z` whose notes are that version's CHANGELOG section.
- Installed copies look for newer releases by their tags, so a tag above the current version must
  never exist. Pre-releases below 1.0.0 on GitHub are an archive; they are not mentioned in the
  repository.

### Documentation

- `README.md` is for people using the program: what it does, installing, using it, settings,
  working with the other programs, limits. Current state only.
- `CHANGELOG.md` follows Keep a Changelog, newest first: `### Added`, `### Changed`, `### Fixed`,
  bullets in the user's terms (a bold lead-in for the important ones), no internal reasons, no
  commit hashes.
- `CLAUDE.md` is for Claude sessions: the same section order in every repository. Keep a reason
  only where it stops a session from undoing a decision.
- Update README and CHANGELOG in the commit that changes the behaviour.

### Wording

British spelling (colour, behaviour, analyse); names of code, GTK and GNOME stay as they are.
Program names are written as above (lower case, except Jewelcase). Terms used in all six:

| Use | Not |
|---|---|
| folder | directory (except in code and option names) |
| the page (the program's page in the browser) | web UI, frontend, site |
| settings | options, preferences (except GNOME's *Preferences* window in the GTK apps) |
| dry run (`--dry-run`) | simulation, test run |
| the trash; *move to the trash* | recycle bin, wastebin |
| the owner (in CLAUDE.md) | the user, the maintainer, a name |
| you (in README) | the user |
| Enter, Esc, Ctrl+C | ENTER, Return, ^C |

## Layout

```
app/        GTK interface (crate `jewelcase`): windows, widgets, tools.rs (companion tools),
            updates.rs (update banner), rename.rs (migration from the old name)
core/       library, scanner, queue, playback, similarity, matching; no GTK
lyrics/     embedded and sidecar lyrics, LRC, romanization pairing
metadata/   MusicBrainz, Wikidata, Commons and Deezer lookups
spotify/    Spotify login (OAuth PKCE) and Web API
soloist/    Spotify Soloist over its local WebSocket API, and soloistd
stream/     optional librespot streaming (feature `spotify-stream`)
tools/      lyrembed, cdport and tag editors: commands, configs, D-Bus names
data/       desktop file, metainfo, settings schema, icons, UI (data/resources: Blueprint, CSS)
po/         translations
build-aux/  headless.sh (test session), flatpak/ (test Flatpak manifest), cargo-build.sh
```

- Meson builds the UI resources (Blueprint, CSS, icons) and runs Cargo, which embeds the GResource
  bundle: a binary built by Cargo alone won't start (`cargo check`/`clippy` work on their own).
- App ID `io.github.sobakah.Jewelcase`; development builds are `io.github.sobakah.Jewelcase.Devel`
  (own data, settings and keyring entries; striped header bar; the version shows the commit,
  "-dirty" with uncommitted changes).
- Target: Fedora 45 (GNOME 51, GTK 4.24, libadwaita 1.10). Other distributions are meant to come
  through the Flatpak, not through lower version floors or native packages.
- Repository: `sobakah/jewelcase` on GitHub, private.

## Commands

Development packages on Fedora 45:

```sh
sudo dnf install cargo rust rustfmt clippy cargo-rpm-macros \
    meson blueprint-compiler gettext desktop-file-utils appstream \
    gtk4-devel libadwaita-devel \
    gstreamer1-devel gstreamer1-plugins-base-devel sqlite-devel \
    fontconfig-devel libseccomp-devel \
    gstreamer1-plugins-good gstreamer1-plugin-libav glycin-loaders bubblewrap \
    mock rpm-build
```

```sh
make build      # meson setup + compile into _build (profile development, -Dspotify_stream=true)
make run        # runs _build/app/jewelcase with GSETTINGS_SCHEMA_DIR=_build/data
make test       # cargo tests + desktop file, metainfo and icon validation (meson test);
                # about 40 s after a small change, much longer after a full rebuild
make lint       # fmt --check + clippy (without the spotify-stream feature)
make rpm        # local rpmbuild into rpmbuild/ (several minutes)
make mock       # clean Fedora 45 chroot (needs the mock group)
make flatpak    # test Flatpak io.github.sobakah.Jewelcase.Devel, installed for this user
```

Before committing, run each and check its exit status:

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets --features spotify-stream -- -D warnings
make test
```

`JEWELCASE_LOG=debug` (or `trace`, `warn`) sets the log level; the log goes to the journal
(`journalctl --user -t jewelcase`, with hourly Spotify request counts per kind).

## Rules that must not break

- **The owner's data is read-only**: the library database in
  `~/.local/share/io.github.sobakah.Jewelcase/`, the settings, soloistd's config, the Spotify
  account. Never write to them in tests; open the database with `sqlite3 -readonly`.
- **Jewelcase never writes music files**; the companion tools do. Don't rewrite music files
  without asking.
- **Don't read or print secrets**: Spotify tokens, the gh token, the Soloist API key (soloistd's
  `config.json` holds it: read only the keys you need, e.g. `device_name`, `websocket`).
- **Tests that open windows or play audio run in `build-aux/headless.sh`** (see Testing). If
  something can only be tried on the real desktop or speakers, warn and ask first.
- **Don't touch `~/.local/share/dbus-1/services`**: the owner's real service files for cdport
  and lyrembed.
- **Keep the migration from the old name, Gatefold** (installations of Gatefold still exist):
  - `app/src/rename.rs`, on the first start: moves Gatefold's data, cache and config folders to
    the new app ID, copies its dconf settings with the `dconf` CLI while Jewelcase has none (only
    with the dconf backend), swaps a pinned dash launcher. Not in a Flatpak.
  - `app/src/spotify/keyring.rs`: copies keyring entries from the old app ID once (marker file
    `keyring-renamed-<kind>` in the data folder); a disconnect also sets the marker, so an old
    login can't come back.
  - `jewelcase.spec`: `Provides`/`Obsoletes: gatefold` and the `/usr/bin/gatefold → jewelcase`
    link, because Gatefold's updater restarts with `exec gatefold`.
  - `app/src/tools.rs` `migrate_cdimport_settings` and the `cdimport-*` schema keys, and
    `tools/src/cdport.rs` reading `~/.config/cdimport/config.json`: cdport's old name.
  - None of this goes into the README or the CHANGELOG.
- **Keep `_build/target`** between commits (it's tens of GB). Only suggest deleting it above
  ~20 GB or at a release, and ask.
- Don't propose lowering the GTK/libadwaita requirement or native packages for other
  distributions (the Flatpak is the route).

## Conventions

- Commits go directly on `main` (history is linear) and end with
  `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`.
- **Change UI elements everywhere they appear**, unasked: grips, panel slides, buttons. Grep for
  the style class or widget (`.grip`, `SlideView`, `panel-close-handle`, `sheet-close-handle`) and
  bring every place in line, or say why one can't follow.
- Tooltips: common symbols (play, pause, next, shuffle, repeat, volume, close, ⋮) have no tooltip,
  only an accessible label (`widgets::set_accessible_label`); less common icons keep one, with the
  key combination if there is one. Tooltips wait until the pointer rests for a second
  (`widgets/tooltips.rs`).
- Settings are GSettings keys in `data/io.github.sobakah.Jewelcase.gschema.xml.in`.
- Companion tools (`app/src/tools.rs`, `tools/`): cdport and lyrembed are used over D-Bus whenever
  their `GetInfo` answers, else as programs (found in `PATH`, then `~/.local/bin`). Features are
  detected (`lyrembed romanize --help` for `--stdio`), not read from version numbers.

## Testing

- **Headless session**: `build-aux/headless.sh COMMAND` gives a headless mutter, its own D-Bus
  session, in-memory GSettings, silent audio (`JEWELCASE_SILENT=1`), no browser, no keyring, and
  its own data in `_build/headless/` (or `$JEWELCASE_HEADLESS_STATE`). Always set
  `GSETTINGS_SCHEMA_DIR=$PWD/_build/data`.
  ```sh
  GSETTINGS_SCHEMA_DIR=$PWD/_build/data JEWELCASE_SCREENSHOT=<scratchpad>/shot.png \
      build-aux/headless.sh _build/app/jewelcase
  ```
- **Screenshot hooks** (debug builds; all in `setup_screenshot` in `app/src/window.rs`):
  `JEWELCASE_SCREENSHOT`, `_DELAY`, `_SIZE` (with `JEWELCASE_HEADLESS_MONITOR=WxH` for a taller
  screen: the mini player covers the bottom), `_SETTINGS="key=value …"`, `_PLAY=1`, `_ACTIONS`,
  `_LATE_ACTIONS`, `_TIMED_ACTIONS="8:win.x …"`, `_PAGE_ACTIONS` (artist page/album panel actions
  after 4 s), `_ALBUM`, `_ARTIST`, `_PLAYLIST=name`, `_ADD_SONGS=text`, `_ADD_SPOTIFY=1`,
  `_FILTER_SONGS=text` (both 2 s after the playlist opens), `_PREFS=<page>`, `_SEARCH`,
  `_SPOTIFY=1` (connect), `_STREAM=1`, `_EDITS`, `_PUBLISH`, `_DEVICE`, `_IMPORT`,
  `JEWELCASE_SCREENSHOT_UPDATE=1` (the update dialog after the check).
- The headless library scans the owner's music folder **read-only** (through `user-dirs.dirs`).
- **Real clicks headless**: mutter's `org.gnome.Mutter.RemoteDesktop` on the private bus. The
  session dies with its caller, so use one process (Python `Gio.DBus`): `CreateSession`, `Start`,
  `NotifyPointerMotionRelative(-10000,-10000)` then `(x,y)`, `NotifyPointerButton(272, true/false)`.
- **Spotify without the account**: debug builds talk to
  `JEWELCASE_SPOTIFY_TEST_SERVER=http://127.0.0.1:<port>` and complete the login by themselves.
  The stand-in server is a throwaway Python script in the scratchpad (not in the repo):
  `POST /api/token`, `GET /v1/me`, `/v1/search` (tracks, artists), `/v1/artists/<id>/albums`, an
  empty page for everything else. A sync against it drops Spotify playlists it doesn't list, so
  use a copy of the headless state (`JEWELCASE_HEADLESS_STATE=<scratchpad>/state`).
- **Soloist and soloistd**: headless runs set `JEWELCASE_SOLOISTD_CONFIG` to the state folder, so
  they never control the owner's real Soloist. Don't connect tests to the owner's soloistd.
- **Updates**: `JEWELCASE_UPDATE_TEST=<version>` pretends to be that version (and checks in
  development builds); `JEWELCASE_UPDATE_REPO=owner/name` asks a public repository instead.
- **dconf instead of memory settings** (e.g. to test settings migration): a copy of headless.sh
  without `GSETTINGS_BACKEND=memory` and with `XDG_CONFIG_HOME=<scratchpad>/config`; read the
  result with `XDG_CONFIG_HOME=… dconf dump /io/github/sobakah/`.
- **Companion tools over D-Bus**: a private service file in the headless state's
  `data/dbus-1/services/` with `Exec=/usr/bin/env XDG_CONFIG_HOME=<scratch>/config … --dbus`.
  lyrembed falls back to writing real files when a lyrics search is outside its music folder: use
  a scratch music folder and `lyrembed-path='/nonexistent/…'`. Afterwards remove the service files
  and stray processes (by PID).
- Things headless can't show: alternating frames from real input. For "doubled"/flicker reports,
  ask for a photo of the screen; never put a drag gesture on a widget that moves with the drag.

## Releasing

1. Raise the version in `Cargo.toml` (workspace), `meson.build` and `jewelcase.spec`; run
   `cargo update --workspace` (only the workspace crates may change in `Cargo.lock`).
2. `CHANGELOG.md`: `## [Unreleased]` becomes `## [X.Y.Z] — YYYY-MM-DD` (the link list at the end
   gets the new version). Say whether the database changes and whether the previous version still
   opens it. Add a `<release>` to the metainfo and an entry to the spec's `%changelog`.
3. `cargo fmt --all --check`, clippy, `make test`, and a local `make rpm` when packaging changed
   (`dnf install --assumeno <rpm>` shows what dnf would do, without root). Remove the local test
   RPMs afterwards.
4. Commit `Jewelcase X.Y.Z: <what it brings>`, `git tag -a X.Y.Z -m "Jewelcase X.Y.Z"`, push
   `main` and the tag.
5. The tag starts `.github/workflows/rpm.yml` (plain version tags only; it checks the tag against
   `meson.build`): it builds the RPMs in a clean Fedora 45 container (20–30 min) and creates a
   **draft** pre-release titled "Jewelcase X.Y.Z" with the version's CHANGELOG section as notes.
   Check it (`gh release view X.Y.Z`, download the RPM and inspect it), then publish it with
   `gh release edit X.Y.Z --draft=false --prerelease`. Don't also create the release by hand: that
   leaves a duplicate draft.
6. Installed copies find the release through their update banner (`app/src/updates.rs`: GitHub
   releases via `gh api` or HTTPS, tags with or without `v`, any `*.x86_64.rpm` asset, installed
   with `pkexec dnf install`, restarted with `exec jewelcase`). The banner is off in development
   builds and Flatpaks.
7. Before a *public* release (Flathub, public repository): check the icon and texts for anything
   borrowed.

## Gotchas

- Without a host `flatpak-builder`, `make flatpak` uses the Flathub app `org.flatpak.Builder`; its
  warning "Unknown bus method ReloadConfig" at the end is harmless (its sandboxed D-Bus may only
  talk to `org.freedesktop.Flatpak`).
- Flatpak's icon check recognizes an SVG by `<svg` near the file's start: nothing long (a comment)
  may come before the `<svg>` element. `make test` checks both app icons.
- Rendering SVGs for review: Python with `gi` `Rsvg` 2.0 and cairo works. Compare app icons against
  `/usr/share/icons/hicolor/scalable/apps/org.gnome.*.svg` at 64 px.
- GitHub redirects `sobakah/gatefold` to `sobakah/jewelcase`, also for the API, so old installs
  find new releases. `releases/latest` answers 404 while every release is a pre-release.
- Logs of Gatefold builds are under `journalctl --user -t gatefold`.
- soloistd's `device_name` is the owner's config; don't change it unasked.
- R128 gain tags are ignored when ReplayGain tags exist (some cyanrip rips have R128 tags 10 dB
  too high). Some M4A files stop decoding partway in plain GStreamer too: check with
  `gst-launch-1.0` before calling it a Jewelcase bug.
- **Spotify Web API**: the owner's own client ID (Preferences → Spotify has the steps; redirect URI
  `http://127.0.0.1:8974/callback`). Each client ID has its own rate limits. Development Mode
  refuses followed playlists' items (403).
- Spotify blocks `/artists/{id}/albums` for about 21 h after roughly 20 discographies a day.
  Discographies are kept a week (`DISCOGRAPHY_AGE_S`); artist pages say when they were read and
  can ask again. When the owner reports a block: read the journal (hourly request counts per kind)
  and the read-only database.
- **Streaming through Spotify Soloist** (default): Jewelcase attaches to soloistd's Soloist
  (address from `websocket` in `~/.config/soloistd/config.json`), else to a running Soloist, else
  starts one itself with the key from the keyring. **Spotify gives one Soloist API key per
  account**: the installed app's soloistd and the test Flatpak share it. Preferences → Spotify →
  Streaming has step lists for setting it up, with or without soloistd
  (`app/src/spotify/streaming_guide.rs`).
- The test Flatpak reads soloistd's config (`--filesystem=xdg-config/soloistd:ro`) and uses the
  same Soloist as the installed app; it falls back to running soloistd's downloaded Soloist itself
  (`xdg-data/soloistd/bin:ro`). Both apps then control one Spotify device: whichever plays last
  has it.
- librespot (`stream` crate) is the unofficial alternative; Spotify refuses its audio keys on some
  accounts (librespot issue #1649). `Cargo.lock` pins vergen 9.0.6 for librespot-core's build
  script.
- Jewelcase only asks LRCLIB for lyrics of Spotify songs while the lyrics are on screen.

## Open items

- The artist page has two ↻ icons with different jobs ("Look Up Again" for the artist's
  information at the top, "Ask Spotify Again" for the releases); the owner chose to leave it for
  now.
- Clicking beside the open player once stopped closing it until a restart; not reproduced
  (libadwaita ignores that click while it thinks a swipe on the sheet is still going). Ask what
  was done just before if it happens again.
- Not verified: two Soloists running on one API key at the same time; Soloist's WebSocket with two
  connected apps at once.
- Known issues left from the code reviews: CONCEPT.md §13 "Known issues". Roadmap and optional
  features: CONCEPT.md §13; open questions: §14.
