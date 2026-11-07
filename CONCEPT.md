# Gatefold: Concept Draft

*Status: draft v0.3 (2026-11-05). Target: Fedora 45 (GNOME 51, GTK 4.24, libadwaita 1.10, GStreamer 1.28).*

### Decisions so far
| Topic | Decision |
|---|---|
| Name | **Gatefold**, app ID `io.github.sobakah.Gatefold`, command `gatefold` (§12) |
| Language / library | Rust; own scanner + SQLite |
| Album details | Side panel (`AdwOverlaySplitView`); only artists get their own page |
| Metadata source | **Local tags are the source of truth.** Online data only adds what tags can't hold (portraits, links, bios). |
| Spotify | Links, remote control, streaming (librespot), **Spotify playlists with local substitution**, mixed playlists, **two-way editing** of owned Spotify playlists (§9.6) |
| lyrembed / cdimport | Started as external programs; their pages open **in the default browser**; no GTK port planned |
| Romanization | **Automatic**, using lyrembed's engine (§5.1) |
| Audio analysis | Wanted (bliss + BPM detection, §7) |
| Album editions | Not part of the title. Shown only as a subtitle in the album panel; same-tracklist editions merged (§3.4) |
| Grids | Library album grid: cover, album, artist. Artist page: cover, album (§4.1, §4.2) |
| "Appears on" | Yes, collapsed, only for artists with their own album-artist entry (§4.2) |
| Tag editing | Not in the player; delegated to **Ear Tag** (§10.3) |
| Scrobbling | No |
| Packaging | **Local RPM only** for now; no Flatpak, no distribution (§12) |
| Symlinks | Not allowed for now (§3.1). The rclone-synced Google Drive copy is not part of the library. |
| Joint album artists | Trust the tag, link without splitting (§3.3) |

## 1. Vision

A local-first music player for people who listen to **albums**, follow **artists**, and curate **playlists**.

- **Sleek by default.** The first thing you see is cover art and very little chrome.
- **Expandable.** Lyrics, the queue and details can be opened when you want them, and they stay out of the way when you don't.
- **Cover art first.** Covers are the main navigation element and the main visual of the player.
- **Clean artist list.** Only album artists get entries. Featured or collaborating track artists don't create clutter.
- **Built, not reinvented.** Every subsystem uses an established, maintained library or protocol.

Non-goals for v1: tag editing inside the player (delegated to Ear Tag), online lyrics, scrobbling, podcasts, a "Songs" mega-list as a primary view, Flatpak/Flathub distribution, and Windows/macOS support.

---

## 2. Technology Stack

| Concern | Choice | Why |
|---|---|---|
| Language | **Rust** (stable) | Same stack as Amberol, Loupe, Euphonica and Fractal. Memory-safe, with good async support. |
| UI toolkit | **gtk4-rs + libadwaita-rs** | Required design language. libadwaita 1.10 has every adaptive widget we need. |
| UI definitions | **Blueprint** (`blueprint-compiler`) → GtkBuilder | The GNOME-standard declarative UI format. Easier to read than raw XML. |
| Build | **Meson + Cargo** (GNOME Rust app template) | Standard GNOME build setup. Handles gettext, GSettings schemas, resources and icons. |
| Playback | **GStreamer 1.28 via gstreamer-rs**, `playbin3` | Proven codec support, gapless playback (`about-to-finish`), ReplayGain (`rgvolume`), equalizer. Fedora ships the needed codecs. |
| Tag reading | **lofty**, with **GStreamer Discoverer** as fallback | lofty reads ID3v2 (including `SYLT`/`USLT` lyrics frames), Vorbis comments, MP4 (incl. freeform `----:` atoms and `xid `), APE and FLAC pictures, plus MusicBrainz IDs and multi-value tags. lofty can't read ASF, so WMA files (not in `~/Musik` now, but in the Google Drive copy) get basic tags from GStreamer's Discoverer. Fedora 45 has `asfdemux` and `avdec_wmav2`, so they play. |
| Database | **SQLite via rusqlite** + `rusqlite_migration`, FTS5 for search | Embedded and robust. FTS5 gives fast full-text search. |
| File watching | **notify** crate (inotify) | Recursive watching works better here than with `GFileMonitor`. |
| Image loading | **glycin** (GNOME's sandboxed image loader, used by Loupe) | Safe decoding of untrusted embedded covers. Fedora ships it. |
| Color extraction | `kmeans_colors` / `color-thief` | Player accent and background colors taken from the cover. |
| HTTP | **reqwest** on a background Tokio runtime, bridged to the GLib main loop with `async-channel` | Standard approach in gtk-rs apps. |
| Metadata | **musicbrainz_rs**, Wikidata REST/SPARQL, Cover Art Archive, fanart.tv | Free, open and ID-based, with no scraping. |
| Audio similarity | **bliss-audio** (bliss-rs) | Local song analysis (tempo, timbre, chroma, loudness) that gives a distance metric. Spotify's audio-features API is no longer available, so this replaces it. |
| BPM detection | GStreamer **`bpmdetect`** (SoundTouch, installed on Fedora 45) | The reference library has **no BPM tags at all**, so BPM has to be measured. This runs in the same analysis pass as bliss. |
| Desktop integration | **mpris-server** crate | Media keys, GNOME Shell media controls, KDE Connect. |
| Secrets | **oo7** (GNOME's Rust Secret Service client) | Stores Spotify tokens in the keyring. |
| Settings | **GSettings** | Standard GNOME settings storage, available through gio. |
| Spotify streaming | **librespot** (optional Cargo feature) | The only maintained open-source Spotify client library. See §9. |
| Companion tools | **gio::Subprocess** (async, GLib-integrated) | Runs lyrembed, cdimport and Ear Tag as separate processes. The tools' pages open in the default browser. See §10. |
| Packaging | **RPM** for Fedora 45, built locally (`rpmbuild` / `mock`) | Personal use only for now. Native packaging keeps direct file access and lets the app start lyrembed, cdimport and Ear Tag without sandbox workarounds. See §12. |

### Crate / module layout (Cargo workspace)

```
gatefold/           # this repository (Cargo workspace root)
├── app/            # GTK UI: windows, widgets, Blueprint files, GObject wrappers
├── core/           # no GTK dependency: library DB, scanner, queue, playback engine
├── metadata/       # MusicBrainz, Wikidata, fanart.tv, Cover Art Archive providers
├── lyrics/         # LRC/embedded parsing, dual-line pairing
├── similarity/     # bliss analysis worker + "similar track" queries
├── tools/          # lyrembed / cdimport: detection, version probing, process control
└── spotify/        # feature-gated: Web API client, librespot integration
```

`core` exposes plain Rust types and async channels. `app` wraps them in GObject subclasses (`TrackObject`, `AlbumObject`, `ArtistObject`) stored in `gio::ListStore` for GtkListView/GtkGridView. This keeps the UI layer thin and makes `core` easy to test.

---

## 3. Library Model

### 3.1 Scanning
- The user picks one or more music folders. The default is the XDG Music dir (`~/Musik`). The app runs natively, so folders are read directly.
- The initial scan runs on a worker thread pool. lofty reads tags, and results are written to SQLite in batches. Progress shows in an `AdwBanner`.
- Incremental updates come from inotify events. A periodic mtime check catches anything that was missed.
- The library is **read-only for v1.** The app never writes to music files.
- **Symlinks are not allowed (for now).** The scanner skips every symlink, whether it points to a file or a folder, and the folder chooser refuses a symlinked folder as a library folder, with a short explanation. In the reference library this ignores `~/Musik/Verknüpfung mit Musik@GDrive → ~/Google Drive/Musik`, an rclone-synced Google Drive copy of the library that would otherwise show every album twice. Files are identified by canonical path, so overlapping library folders (one inside another) are rejected too.

### 3.2 Schema (simplified)
```
artist(id, name, sort_name, mbid, portrait_path, portrait_source, spotify_id, wikidata_id)
album(id, title, sort_title, year, release_type, mbid, release_group_mbid, cover_path, dominant_color)
album_artist(album_id, artist_id, position)        -- the ONLY source of artist-list entries
track(id, album_id, path, title, disc, number, duration, bpm, genre[], mbid,
      track_artist_display, lyrics_embedded bool, lyrics_sidecar_path, bliss_vector BLOB)
track_artist(track_id, artist_id, role)           -- 'main' | 'featured'; used for "Appears on" only
playlist(id, name, cover_path, created, modified)
playlist(…, source)                               -- 'local' | 'spotify' (mirror, §9.5)
playlist_item(…)                                  -- local track and/or Spotify URI, see §9.5
queue_state(...) / queue_item(...)                -- see §6
```

### 3.3 Album-artist rule (keeps the artist view clean)
1. An artist gets an entry in **Artists** only if they appear in `album_artist`.
2. The album-artist value comes from `ALBUMARTIST` / `TPE2` / `aART`. If that tag is missing, the track `ARTIST` of the majority of the album's tracks is used.
3. **Joint credits (Q1, recommendation): trust the tag, never split strings.** The tag decides whether an album artist is one entry or several:
   1. **`ALBUMARTISTS`** (multi-valued, Picard convention: `ALBUMARTIST` holds the credit line, `ALBUMARTISTS` the single names) or **`MUSICBRAINZ_ALBUMARTISTID`** present → one artist per value, and the album is listed under each of them. cdimport's tag editor writes `ALBUMARTISTS`, and Ear Tag or Picard can set it too.
   2. Otherwise the `ALBUMARTIST` string is **one artist entry, exactly as tagged**. "Get Well Soon & Kat Frankie" stays one entry, and "Simon & Garfunkel" can never break.
   3. **Linking without splitting**: if a part of a joint credit (split on ` & `, `, `, ` × `, ` / `, ` and `, ` und `. Deliberately not on a plain "x", because of "TOMORROW X TOGETHER") **exactly matches another existing album artist**, the joint album also shows up on that artist's page in a *Collaborations* section. No new artist entries are created from parts. This is a display link only.
   4. To really split an entry, fix the tag (set `ALBUMARTISTS`). The artist page offers *"Edit album artist tags…"*, which opens that artist's albums in the configured tag editor (Ear Tag by default, §10.3).
4. **Identity is case- and alias-insensitive**: "YEONJUN" (album artist) and "Yeonjun" (track artist on TXT albums) are the same artist. Spellings in the same lyrembed alias group (§10.1, e.g. "TXT" = "TOMORROW X TOGETHER" = "투모로우바이투게더") are merged. The **display name is the most frequent album-artist spelling**. MBIDs take precedence when present.
5. "Various Artists" (MBID `89ad4ac3-39f7-470e-963a-56509c546377`) or the `COMPILATION=1` flag → shown under a **Compilations** filter instead of as an artist entry.
6. Featured and track artists are stored in `track_artist` but never create an artist entry. Track-artist strings in the reference library are messy ("RM (feat. eAeon)", "Alligatoah, Battleboi Basti", "Bang Chan, Changbin, HAN", "RM  (prod. HONNE)"). They're shown as tagged and only parsed for "Appears on" links to **existing** album artists, using the same exact-match rule as 3.3.

### 3.4 Album identity, editions and release types (from local tags)
- **Editions are not part of the album name.** K-pop album versions ("L version", "Day ver.", "MASK OFF ver.", stored in **`RELEASECOMMENT`**, 144 tracks) have the same music and differ only in packaging. The edition is therefore shown **only as a subtitle in the album panel**: "LOVE YOURSELF 結 'Answer'" with *L version* underneath. Grids, search, the artist page, MPRIS and Spotify matching show and use the clean title only.
  - The source is `RELEASECOMMENT`. Where the edition is only written into the title, recognized suffixes are moved out of the displayed title into the subtitle (the tag itself is never changed): `(… ver.)`, `(… version)`, `(Deluxe)`, `(Deluxe Version)`, `(Remastered)`, `(Special Edition)`. Examples from the library: "Blue Neighbourhood (Deluxe)" → "Blue Neighbourhood" · *Deluxe*, and "Floral Shoppe (Remastered)" → "Floral Shoppe" · *Remastered*.
  - **Several editions of the same album** (same title + album artist, and the **same tracklist**: titles and durations ±2 s) are merged into **one** album. The panel footnote lists the versions you have ("Versions: L version, F version"). Playback uses the first version (or the one chosen in the panel), and the others are kept as alternatives. If the tracklists differ (e.g. a Deluxe edition with bonus tracks), the editions stay separate albums, each with its subtitle.
  - The album identity is `MUSICBRAINZ_ALBUMID` if present, else album + album artist + folder. The merge above sits on top of this as an "album group".
- **Release type**: no file has `RELEASETYPE`. Local rules, in order:
  1. `RELEASETYPE` tag if present.
  2. iTunes title suffix ` - Single` / ` - EP`. The suffix is **stripped for display** and used as the type, e.g. "Ich hab Polizei - Single" → "Ich hab Polizei" · *Single*.
  3. Otherwise the track count/length rule streaming services use: 1–3 tracks and < 30 min → Single, ≤ 6 tracks and < 30 min → EP, else Album.
  4. Genre `Soundtrack(s)` → also tagged *Soundtrack*, which makes a filter and an artist-page section.
- **Genre normalization** (for grouping and the genre shuffle criterion only, never shown altered): case-fold, trim (`"J-Pop "`), unify punctuation (`Hip-Hop/Rap` = `Hip Hop/Rap`), singular/plural (`Soundtrack` = `Soundtracks`).

### 3.5 Reference library: what the tags contain
A survey of `~/Musik` on 2026-11-05 (596 tracks: 333 FLAC, 190 M4A, 73 MP3) is the basis for these defaults:

| Field | Present | Consequence |
|---|---|---|
| `ALBUMARTIST` / `aART` / `TPE2` | 100 % | Album-artist rule works without fallbacks. Only 2 of 33 album artists are joint credits. |
| `ALBUMARTISTS`, `MUSICBRAINZ_ALBUMARTISTID` | 0 % | Joint credits stay one entry (§3.3) until tags are fixed. |
| `MUSICBRAINZ_ALBUMID`, `_TRACKID`, `_DISCID` | FLAC rips (cyanrip) | Exact album identity, portrait/link lookup via release → artist without guessing. |
| ISRC | M4A `xid ` atom (`label:isrc:DEQ021801392`), 129 tracks | **Local ISRCs for matching Spotify playlist entries** (§9.5). |
| `RELEASECOMMENT` | 144 tracks | Edition subtitle in the album panel, not part of the title (§3.4). |
| ReplayGain + R128 | FLAC rips | `rgvolume` uses them. Album gain is the default in album context, track gain in shuffle. |
| BPM | 0 % | BPM measured by `bpmdetect` (§7). |
| Genre | 98 % (K-POP 285, Soundtrack(s) 181, Hip-Hop/Rap 64, Pop 43, …) | Too coarse alone for "similar" (half the library is K-POP), so it's combined with bliss + BPM. |
| Lyrics | 373 tracks (362 synced). **254 already dual-line** (lyrembed "below" layout). No sidecars. | Embedded lyrics are the main source. Auto-romanization (§5.1) covers the rest. |

---

## 4. UI Structure

Two top-level modes: **Library** and **Player**. They're connected through libadwaita's `AdwBottomSheet`, a mini-player bar at the bottom of the library that slides up into the full player. This is the standard pattern for music players in libadwaita ≥ 1.6.

### 4.1 Library window

```
┌──────────────────────────────────────────────────────────────────────┐
│ ☰  [ Albums | Artists | Playlists ]            🔍   ⋮               │  ← AdwHeaderBar + AdwViewSwitcher
├───────────────────────────────────────────────┬──────────────────────┤
│ ┌──────┐ ┌──────┐ ┌──────┐ ┌──────┐           │ ┌──────────────────┐ │
│ │      │ │      │ │ ▓▓▓▓ │ │      │           │ │                  │ │
│ │      │ │      │ │ ▓▓▓▓ │ │      │           │ │   LARGE COVER    │ │
│ └──────┘ └──────┘ └──────┘ └──────┘           │ │                  │ │
│ Album    Album    Album    Album              │ └──────────────────┘ │
│ Artist   Artist   Artist   Artist             │ Album Title          │
│                                               │ L version (edition)  │
│                                               │ Artist · 2021 · 48min│
│ ┌──────┐ ┌──────┐ ┌──────┐ ┌──────┐           │ [▶ Play] [⤮] [+ ⋮]  │
│ │      │ │      │ │      │ │      │           │ 1  Song ........ 3:12│
│ └──────┘ └──────┘ └──────┘ └──────┘           │ 2  Song ........ 4:01│
│                                               │  ▾ expanded details  │
│                                               │ 3  Song ........ 2:58│
├───────────────────────────────────────────────┴──────────────────────┤
│ [cov] Song Title — Artist          ⏮  ⏯  ⏭     ━━━━●━━━━   ⤢        │  ← AdwBottomSheet bottom bar
└──────────────────────────────────────────────────────────────────────┘
```

- **Albums**: `GtkGridView` of covers. Each tile shows **only cover, album title and album artist** (one line each, ellipsized). There's no size slider: the cover size adapts to the window width, so the columns always fill the row. Sortable by artist, title, year or date added, with filters (release type, genre, soundtracks, compilations). Clicking a cover opens the **detail side panel** (`AdwOverlaySplitView`, sidebar on the end side). Nothing is pushed as a new page.
- **Album detail panel**: large cover, title, edition subtitle (§3.4, plus the list of owned versions when editions were merged), album artist(s) as links (→ artist page), year, length, actions (Play, Shuffle, Add to queue, Play next, Add to playlist). Below that is the tracklist, grouped by disc.
- **Songs expand in place**: clicking a track row's chevron opens a `GtkRevealer` under the row with track artists/featuring, composer, genre, BPM, format/bitrate/sample rate, a lyrics-available indicator with a short excerpt, and actions (Play next, Add to queue, Add to playlist, Go to artist). Double-click or Enter plays the track.
- **Narrow windows** (`AdwBreakpoint` < ~700 px): the side panel turns into an overlay sheet that covers the grid, and the mini bar stays visible.
- **Artists**: grid of circular portraits (`AdwAvatar`-style, with the actual image) and names, using only album artists. Clicking one pushes the **artist page** onto an `AdwNavigationView`. This is the **only** real page navigation in the app.
- **Playlists**: grid of playlist covers (custom image or an automatic 2×2 cover mosaic). Clicking opens the same detail side panel with an editable track order (drag & drop, remove, rename).
- **Search**: a single search field (`GtkSearchBar`) backed by SQLite FTS5. Results are grouped into Artists / Albums / Songs / Playlists in a popover-like list.

### 4.2 Artist page

```
┌──────────────────────────────────────────────────────────────────────┐
│ ←  Artist Name                                              ⋮        │
├──────────────────────────────────────────────────────────────────────┤
│  ╭────────╮   Artist Name                                            │
│  │PORTRAIT│   12 albums · 148 songs                                  │
│  │        │   [▶ Play all] [⤮ Shuffle]                               │
│  ╰────────╯   ⟶ Spotify   ⟶ Wikipedia   ⟶ MusicBrainz   ⟶ Website    │
│                                                                      │
│  Albums ──────────────────────────────────────────────────────────── │
│  ┌────┐ ┌────┐ ┌────┐ ┌────┐                    │  side panel       │
│  Album  Album  Album  Album                       │  (same as in      │
│  EPs & Singles ────────────────────────────────── │   library)        │
│  ┌────┐ ┌────┐                                    │                   │
│  Soundtracks / Live / Compilations ────────────── │                   │
│  Collaborations (joint album credits, §3.3) ───── │                   │
│  Appears on (collapsed by default) ────────────── │                   │
│  On Spotify, not in library (optional, §9) ────── │                   │
└──────────────────────────────────────────────────────────────────────┘
```

- Album tiles show **only cover and album title** (the artist is the page itself). Albums are grouped by release type (§3.4: `RELEASETYPE`, iTunes suffix, track count/length rule, soundtrack genre) and sorted by year.
- Clicking an album opens the same side panel as in the library.
- **"Appears on"** (decided: yes) is a collapsed `AdwExpanderRow`-style section listing tracks where the artist is only a track/featured artist on *other* album artists' albums. It only exists on pages of artists that have their own album-artist entry, and track artists never get a page of their own. Examples from the library:
  - YEONJUN's page lists his solo tracks from TOMORROW X TOGETHER albums.
  - NCT 127's page lists its tracks from the NCT 2018/2020 albums.
  - Rows show the track title and the album it's on. Clicking one opens that album's panel with the track highlighted.
- **Links** are resolved through the artist MBID → MusicBrainz URL relationships and Wikidata (`P1902` = Spotify artist ID, sitelinks for Wikipedia in the user's locale with an English fallback). Spotify links open as a `spotify:artist:<id>` URI if the desktop client is installed, and as `https://open.spotify.com/artist/<id>` otherwise.
- Optional: a short bio from the Wikipedia REST summary endpoint (`/page/summary`), collapsed to two lines.

### 4.3 Player view (the expanded bottom sheet)

Default (sleek):
```
┌──────────────────────────────────────────────┐
│ ⌄                                      ⋮     │
│                                              │
│          ┌──────────────────────┐            │
│          │                      │            │
│          │      COVER ART       │            │  ← background: blurred cover +
│          │     (as large as     │            │    extracted dominant color
│          │      possible)       │            │
│          └──────────────────────┘            │
│              Song Title                      │
│        Artist — Album (links)                │
│   1:23 ━━━━━━━━━●━━━━━━━━━━━━━━━━━━ 3:58     │
│        ⤮      ⏮      ⏯      ⏭      ↻         │
│                       [💬 Lyrics] [☰ Queue]   │
└──────────────────────────────────────────────┘
```

Expanded: the **Lyrics** and **Queue** toggles open side panes. On wide windows the cover moves left and the pane sits next to it (`AdwMultiLayoutView` switching between "cover only", "cover + lyrics", "cover + queue" and "cover + lyrics + queue" layouts depending on width). On narrow windows the pane replaces the cover and a small cover stays in the header.

- Accent color: the dominant cover color is applied through a scoped CSS provider (`@accent_bg_color`, background gradient) **only inside the player view**, with contrast checked against WCAG AA. The rest of the app follows the system accent.
- Cover sources, in priority order: embedded picture (front) → `cover.*`/`folder.*`/`front.*` in the album folder → (optional, opt-in) Cover Art Archive by release MBID. Cached thumbnails go in `~/.cache/io.github.sobakah.Gatefold/covers/` at 3 sizes (grid, panel, player), decoded with glycin.

---

## 5. Lyrics

**Sources (offline only for v1, priority order):**
1. Sidecar `.lrc` next to the audio file (`Song.lrc`). Sidecars win because they're easier to fix.
2. Embedded synced lyrics: ID3 `SYLT`, ID3 `USLT` containing LRC timestamps, Vorbis `LYRICS`/`SYNCEDLYRICS` containing LRC, MP4 `©lyr` containing LRC. Each container is checked for LRC timestamps before being treated as plain text. This matters because **lyrembed writes synced lyrics as LRC into `USLT`/`LYRICS`/`©lyr`** and removes `SYLT` when it writes.
3. Embedded/sidecar unsynced: ID3 `USLT`, plain `LYRICS`, `.txt` sidecar → shown as static, scrollable text.

The provider interface is designed so an online source (e.g. LRCLIB) can be added later as a fourth, opt-in priority without changing the UI.

**Dual-line (original + romanization).** Every lyric line is a pair `{ time, primary, secondary? }`. Romanization is detected from these established conventions:

| Convention | Example |
|---|---|
| **Separate sidecar** with a language/script suffix | `Song.lrc` + `Song.ja-Latn.lrc` (BCP-47 script subtag) or `Song.romaji.lrc` / `Song.roman.lrc` |
| **Duplicate timestamps** in one LRC file (common in CJK LRC files, and exactly what **lyrembed writes with `romanization_layout: "below"`**) | `[00:12.30]夜に駆ける` / `[00:12.30]Yoru ni kakeru` |
| **Multiple embedded frames** | Two `SYLT`/`USLT` frames that differ in content descriptor or language, or a Vorbis `LYRICS` + `LYRICS-ROMANIZED`/`LYRICS:ja-Latn` |

Pairing rule: lines with identical (±50 ms) timestamps are merged. The line in the non-Latin script becomes primary (detected with the `unicode-script` crate), and the Latin one becomes secondary. If both lines are in the same script, file order decides: the first line is the original, as lyrembed writes it. Only lines that lyrembed actually changed get a second line, so a pair can be missing (e.g. an English line in a K-pop song). The UI must handle single lines inside dual-line lyrics. A setting decides which is shown large: "Original on top / Romanization on top / Original only".

**Display:** `GtkListView` of centered lines. The active line is highlighted and auto-scrolled smoothly (`AdwSpringAnimation`). Clicking a line seeks to it. Word-level highlighting works when the file uses enhanced LRC (`<mm:ss.xx>` word stamps). Auto-scroll pauses for 3 s after a manual scroll.

**Instrumental tracks** show "Instrumental" instead of "No lyrics" when the language tag is `zxx` or the title has a version note like `(Inst.)`/`(Off Vocal)`/`(MR)`/`オフボーカル`, following lyrembed's rules.

**Getting lyrics that are missing** is delegated to lyrembed (§10.1). The player never searches online itself.

### 5.1 Automatic romanization
The goal: every non-Latin lyric is shown with a romanized line, without the user doing anything. In the reference library this already holds for 254 of 257 non-Latin lyrics. The rest (3 Hangul tracks in single-line layout), plus everything imported later, gets romanized automatically.

**Engine: lyrembed's romanizer, not a Rust re-implementation.** It already handles Hangul → Revised Romanization, Kana/Kanji → Hepburn, Hanzi → Pīnyīn, mixed-script lines split into runs, Latin text left alone, and pykakasi's character-dropping bugs. A second, weaker engine in the player would give different results for the same song.

**Flow:**
1. When lyrics are loaded (playing track **and** the next two in the queue, prefetched), the player checks for non-Latin primary lines without a secondary line.
2. Those lyrics are sent to a long-running **`lyrembed romanize --stdio`** helper (JSON lines in, JSON lines out, writes nothing). This is an addition to lyrembed (§10.4), and it's **required** for this feature. The process is started on first need and stopped after 5 min idle, so Python/pykakasi start-up (~1 s) happens once, not per song.
3. The result is stored in the player's DB, keyed by a **hash of the lyrics text**. If the file's lyrics change (e.g. re-fetched with lyrembed), the hash changes and romanization runs again.
4. The lyrics pane shows auto-romanized lines exactly like tagged ones, with a small "auto" marker in the pane's header menu (not per line).

**Ambiguous Han characters** (valid as Chinese and Japanese, e.g. `東京物語`): lyrembed resolves about 4 in 5 runs by itself. For the rest the player passes a hint, from local metadata only, in this order:
- Kana elsewhere in the lyrics → Japanese
- `LANGUAGE` tag (`jpn`/`zho`)
- the album's other tracks' lyrics
- the setting *"Ambiguous Han characters: Chinese / Japanese"*, default Japanese (the library's Han-script music is mostly Japanese, e.g. 下村陽子)

**Making it permanent** (optional, one click per album): *"Save romanization to files"* runs lyrembed on the album (its bulk action *Romanize embedded lyrics* with layout `below`). This writes the dual-line format into the tags with lyrembed's backups, so other players (Navidrome, Jellyfin, Symfonium) see it too. Until then the files stay untouched.

**Fallback without lyrembed** (not installed, or older than the version with `romanize --stdio`): no auto-romanization. The Integrations page says why and shows the install/update command.

---

## 6. Queue

### 6.1 Model
The queue is separate from the album/playlist it was started from. That source is kept as a **context**, which matters for repeat (§7).

```
queue_state(current_index, position_ms, shuffle_mode, repeat_mode, context_type, context_id)
queue_item(position, track_ref, origin)   -- origin: 'context' | 'user' (Play next / Add) | 'suggested'
queue_item_unshuffled(position, track_ref) -- original order to restore when shuffle is turned off
```

### 6.2 Behavior
- **Play next** inserts after the current track. **Add to queue** appends to the user section, so it plays before the rest of the context (Spotify-style "Next up" vs. "Next from: Album").
- The queue view is a `GtkListView` with drag-and-drop reordering, swipe/right-click remove, "Clear upcoming" and "Save as playlist".
- Items have sections: *Now playing*, *Next up* (user-added), *Next from: \<Album/Playlist\>*. Suggested tracks (§7) have a subtle badge and can be removed individually.
- **Persistence:** queue and state are written to SQLite (debounced, about every 2 s and on shutdown). On start, the queue is restored **paused** at the previous position. Missing files are skipped with a toast. Restoring can be disabled in Preferences.
- Background playback: closing the window while playing can keep playback going (`GApplication` hold + GNOME's Background portal). This is a setting that defaults to *off* (closing quits, and the queue is saved).

---

## 7. Shuffle & Repeat

**Repeat** (cycles with one button, `AdwToggleGroup` in the popover):
- *Off* → *Repeat context* (album/playlist/whole queue) → *Repeat one*.

**Shuffle** is a toggle button with a popover for options:
- **Shuffle tracks**: a Fisher–Yates permutation of the remaining context. The current track stays put, and the unshuffled order is kept so it can be restored.
- **Shuffle albums** (option): album order is randomized but each album plays in order. This fits an album-focused player.
- **Mix in similar songs** (option, off by default):
  - Criteria (multiple choice): ☐ same genre, ☐ similar BPM (± configurable, default ±8 %), ☐ same artist, ☐ *sounds similar* (bliss distance).
  - Rate: after every *N* context tracks (slider 1–5), insert one suggestion.
  - The candidate pool is the local library minus tracks already in the queue and tracks played in the last X hours. Suggestions are scored as a weighted sum of the selected criteria, and the next one is picked at random from the top-k to avoid repetition.
  - **"Keep playing similar music when the queue ends"** is a separate toggle in the same popover.

**Similarity data:**
- Genre (normalized, §3.4) and artist come from tags and are free to use instantly. Genre alone is coarse (half the reference library is "K-POP"), so it works best combined with the other criteria.
- **Analysis pass** (wanted, see Q4), once per track, in a low-priority background worker (nice'd, pausable, resumes after restart, paused on battery):
  - **bliss-rs** → feature vector BLOB (tempo, timbre, chroma, loudness) for *sounds similar*. Only bliss's analysis is used: Gatefold decodes with GStreamer (one pass feeds bliss and `bpmdetect`), so whatever plays can be analyzed and neither FFmpeg nor Symphonia is needed.
  - **GStreamer `bpmdetect`** → BPM number, because there are no BPM tags in the library. A `BPM`/`TBPM`/`tmpo` tag, if one ever exists, takes precedence.
  - Expect roughly 1–3 s CPU per track: about 15–30 min for the current ~600 tracks. Progress shows in Preferences → Library, and features that depend on the analysis are greyed out per track until it's done.
  - Results are keyed by file content hash (not path), so moved or renamed files (cdimport's moves, identical tracks in merged editions) aren't analyzed twice.
- Optional later: ListenBrainz's public similar-artists dataset (by MBID, read-only, no account, no scrobbling) to widen the "same artist" criterion to "related artists". Spotify's related-artists and recommendations endpoints are **not available** to new apps since Nov 2024, so they aren't an option.

---

## 8. Artist Portraits & Online Metadata

**Lookup chain**, run lazily when an artist appears on screen, rate-limited to 1 req/s for MusicBrainz, with a descriptive User-Agent and results cached in the DB:
1. **Local**: `artist.jpg`/`folder.jpg` in the artist directory (Kodi/Jellyfin convention).
2. **User override** (always wins once set).
3. **fanart.tv** `artistthumb` by MBID (good quality, needs a free project API key).
4. **Wikidata `P18`** → Wikimedia Commons image (freely licensed; attribution shown in a tooltip).
5. **Spotify Get Artist** images (only when the user connected Spotify, §9).
6. Fallback: generated initials avatar tinted with the color of their most common album cover.

If tags have no MBIDs, the app searches MusicBrainz by name and album titles and auto-accepts only a high-confidence match. Otherwise the artist shows a small "Identify artist…" prompt where the user picks the right match.

**Customization** (artist page `⋮` menu or right-click on the portrait):
- *Choose image…*: file chooser via portal.
- *Choose from online sources…*: dialog with a grid of candidates from every provider.
- *Adjust crop*: drag to set the focal point used for the circular crop (stored as normalized x/y).
- *Reset to automatic*.

Custom portraits are copied to `~/.local/share/io.github.sobakah.Gatefold/artists/<artist-id>.<ext>`, so they survive rescans.

Online metadata is **opt-in at first launch** (one `AdwStatusPage` explaining what's fetched from where) and can be turned off per provider in Preferences.

---

## 9. Spotify Integration

### 9.1 Current platform constraints (as of Nov 2026)
- **Since Feb/Mar 2026**, Web API apps in Development Mode require the **app owner to have Spotify Premium**, are limited to **5 authorized users**, and have fewer endpoints. `/search` is capped at 10 results. Several batch endpoints, `/artists/{id}/top-tracks` and others were removed. Extended Quota Mode, which has no such limits, is effectively only open to large organizations.
- **Since Nov 2024**, recommendations, audio features/analysis and related artists are unavailable to new apps.
- Still available: `GET /artists/{id}`, `GET /artists/{id}/albums`, `GET /albums/{id}`, `/me/playlists`, `/playlists/{id}/items` (own/collaborative playlists only), `/me/tracks`, and **all `/me/player/*` endpoints**.

**Consequence:** the app can't ship with a shared Spotify client ID. Like `ncspot` and `spotify-player`, it asks the user to register **their own** client ID at developer.spotify.com (a 2-minute guided setup in Preferences). That's fine for personal use, and it's the only sustainable option.

### 9.2 Tier 1: Links (no account needed)
- The Spotify artist ID comes from Wikidata `P1902` or MusicBrainz URL relations. Album IDs come from MusicBrainz URL relations.
- "Open in Spotify" on artist pages and album panels. It uses the `spotify:` URI if a handler is registered, and the web URL otherwise.

### 9.3 Tier 2: Remote control (Web API, OAuth PKCE)
- OAuth Authorization Code + PKCE through the system browser, with a loopback redirect on `http://127.0.0.1:<port>/callback`. Tokens are stored with **oo7** in the GNOME keyring.
- A **"Spotify" device popover** in the player header lists the user's Connect devices (`GET /me/player/devices`). When a remote Spotify device is active, the player view switches into *remote mode*: it shows what's playing (`/me/player/currently-playing`, polled every few seconds) and controls play/pause/next/seek/volume/shuffle/repeat on it.
- Optional: the artist page section "On Spotify, not in your library" (`GET /artists/{id}/albums`).

### 9.4 Tier 3: Streaming via librespot (optional, feature-gated)
- librespot is a Rust library that implements Spotify's client protocol. **Spot** (a GTK4/libadwaita Spotify client) is a working reference for this exact combination.
- **Integration plan:** implement librespot's `audio_backend::Sink` trait to push decoded PCM into a GStreamer `appsrc`. Spotify and local tracks then share **one** output pipeline (same volume, ReplayGain, EQ and output device handling, and MPRIS just works).
- The queue's `track_ref` becomes `Local(track_id) | Spotify(uri)`, so Spotify tracks can sit in the same queue and in local playlists.
- librespot can also act as a **Spotify Connect receiver** (`librespot-connect`): the app shows up as a device in the Spotify phone app, and casting to it plays through the app.
- **Risks, stated plainly:** librespot is unofficial and against Spotify's terms of use for third-party clients. It requires Premium, and Spotify can break it at any time; there have been outages after protocol changes before. It goes in a separate `spotify-stream` Cargo feature, is **disabled by default**, has its own toggle in Preferences, and its failure must never affect local playback. In the RPM it's a build condition (`--with spotify_stream`, §12), so a build without it contains no librespot code at all.
- **Loudness alignment**: Spotify normalizes to about −14 LUFS, while local ReplayGain uses an 89 dB reference (about −18 LUFS). The pipeline applies one target to both (librespot passes its normalization data, and local files use their ReplayGain/R128 tags). Otherwise switching from a local track to a Spotify track in a mixed playlist would jump in volume.

### 9.5 Spotify playlists, mixed playlists and local substitution
**Goal:** Spotify playlists show up next to local ones and play in the player. **If a playlist entry exists in the local library, the local file plays instead of the stream.** Local playlists can contain Spotify tracks.

**Unified playlist entry:**
```
playlist_item(playlist_id, position,
              local_track_id NULL,          -- set when it's a local file or a confirmed match
              spotify_uri NULL,             -- set when the entry came from / exists on Spotify
              isrc, title, artist, album, duration_ms,   -- snapshot for matching and display
              match_state)                  -- 'local' | 'matched' | 'spotify_only' | 'user_pinned_spotify' | 'unavailable'
```
Playback resolves each entry when it's queued: `local_track_id` if set and the file exists → local file, else `spotify_uri` → librespot, else the entry is skipped with a toast.

**Reading Spotify playlists:**
- **Web API** (Tier 2 login): `GET /me/playlists`, then `GET /playlists/{id}/items`. Since Feb 2026, Development Mode apps only get items for playlists the user **owns or collaborates on**. Followed playlists of other users, and Spotify's editorial playlists, list as names but without tracks.
- **With the streaming feature**, librespot's own metadata API (`librespot-metadata` playlist support) can read any playlist the account can see. *To be verified in a spike before relying on it.* The same ToS caveat as §9.4 applies.
- Changes are detected through the playlist's `snapshot_id`, so a playlist is only re-read when it changed. Spotify playlists are refreshed on start and when opened.

**Matching a Spotify entry to a local track (local metadata first):**
1. **ISRC**: Spotify track `external_ids.isrc` (restored in the March 2026 API change) ↔ local ISRC. Locally: the `ISRC` tag (FLAC/ID3 `TSRC`), or the iTunes **`xid `** atom (`label:isrc:<ISRC>`), which 129 M4As in the reference library have. An exact ISRC match is accepted automatically.
2. **MusicBrainz recording**: for FLAC rips with `MUSICBRAINZ_TRACKID` but no ISRC, the recording's ISRCs can be fetched from MusicBrainz **once** and cached. This is opt-in, following the "local first" rule, and off if online metadata is off.
3. **Normalized title + artist + duration**:
   - Title and artist are case-folded, with `feat.` parts and bracketed localized subtitles removed (the same cleaning rules lyrembed uses).
   - Artists are compared through aliases, so "TXT" = "TOMORROW X TOGETHER".
   - Duration must be within ±3 s.
   - **Version notes must agree**: "(Inst.)", "Live", "Remix", "Sped Up", "(Japanese ver.)", "Acoustic" never match the plain version.
   - The same album is a bonus, not required. A single and its album version count as the same recording when the duration fits.
   - Accepted automatically only above a high score. Lower scores become *suggested matches* that the user confirms.
4. **Several local copies** (e.g. the same song on several merged K-pop editions, or on a single and its album) → prefer the copy on the album named in the Spotify entry, else the album edition chosen in the album panel, else the highest quality (FLAC > M4A > MP3).

Match results are cached in the DB and re-checked when the library changes, so a newly imported CD automatically "takes over" its Spotify entries.

**UI:**
- Playlist panel rows show a small source icon per track: local (no icon, the normal case), *Spotify* (stream icon), or *unavailable* (greyed out, e.g. streaming off and no local copy, with "Open in Spotify").
- A header line: *"38 of 50 tracks play from your library"*.
- Row menu:
  - *Use local file…* (pick or override a match)
  - *Always play from Spotify* (`user_pinned_spotify`, e.g. for a different master)
  - *Show in library*
- **Local playlists** accept Spotify tracks by drag & drop from a Spotify playlist, or with "Add to playlist" on Spotify search results.

### 9.6 Editing Spotify playlists (two-way)
**Possible, with limits.** The Development Mode endpoints still include everything needed:
- `POST /playlists/{id}/items` (add, max 100 per call)
- `DELETE /playlists/{id}/items` (remove)
- `PUT /playlists/{id}/items` (reorder with `range_start`/`insert_before`/`range_length`, or replace)
- `PUT /playlists/{id}` (name, description, public/collaborative)
- `PUT /playlists/{id}/images` (cover, base64 JPEG)
- `POST /me/playlists` (create)

Each write returns a new `snapshot_id`. OAuth scopes: `playlist-read-private`, `playlist-read-collaborative`, `playlist-modify-private`, `playlist-modify-public`.

**Which playlists are editable:**
- **Owned or collaborative** → fully editable in the player: reorder by drag & drop, add, remove, rename, change the cover.
- **Others' playlists** (followed, editorial) → read-only, with *"Duplicate as local playlist"* or *"Copy to my Spotify playlists"*. In Development Mode these may not even show their tracks without librespot (§9.5).

**Spotify playlists only hold Spotify tracks.** That's a Spotify limit. The player handles it like this:
- Adding a **local** track to a Spotify playlist first resolves its Spotify version:
  - **ISRC search** (`/search?q=isrc:<ISRC>&type=track`) → added automatically.
  - Otherwise a title + artist search (max 10 results since 2026, enough for this) → added after the user confirms, as with low-confidence matches (§9.5).
  - **No match** → the track can't go into the Spotify playlist. The player offers to add it to a mixed *local* playlist instead.
- Playback still prefers the local file for every entry (§9.5). Editing never changes that.

**Sync model:**
- **Optimistic local edit, then API call.** Each edit becomes an operation (`add uris at pos`, `remove uri at pos`, `move range`, `rename`) that is applied to the local mirror immediately and sent with the last known `snapshot_id`.
- **Conflict handling**: Spotify applies positional operations on top of the snapshot given, but the player still re-reads the playlist after each write batch and compares it with the expected result. If someone else changed the playlist in the meantime (e.g. on the phone), the player takes Spotify's version as the truth, re-applies any pending operations by **URI** (not position), and shows a toast *"Playlist was changed elsewhere — merged"*.
- **Offline / API errors**: operations wait in a persistent outbox (SQLite) and are sent when the connection is back. 429 responses are retried after `Retry-After`. A playlist with pending changes shows a small sync indicator.
- **Batching**: drag & drop of many items and bulk adds are combined into as few calls as possible (≤ 100 items each).

**Publishing a local playlist to Spotify** (*"Create on Spotify"*): `POST /me/playlists`, then add the resolved URIs. Entries without a Spotify match are listed in a dialog before publishing. The local playlist and the new Spotify playlist stay **linked** (two-way as above), and local-only entries stay only in the local version.

**Effort**: moderate. The sync engine (outbox + re-read + re-apply by URI) is the main part, about the same size as reading playlists. The reverse matching reuses §9.5. It fits into Phase 7 without a separate phase.

**Export:** XSPF keeps both (`<location>` file path + `<identifier>` Spotify URI/ISRC). M3U8 exports local entries and lists Spotify-only entries as comments, with a warning.

### 9.7 As built (phase 7)
- **Login**: the loopback redirect uses a fixed port, `http://127.0.0.1:8974/callback`, since the URI must be registered exactly. The access token is refreshed by itself; tokens live in the keyring (oo7), the account name in GSettings.
- **Linked playlists**: Spotify mirrors and published local playlists work the same way. Entries with a Spotify URI are synced; library songs without a Spotify version stay as "only in Gatefold" entries in any linked playlist (rather than being refused), and keep their place after the Spotify entry they followed when the playlist is re-read.
- **Removing**: Spotify removes every occurrence of a URI, so a remove puts the other occurrences back.
- **Reading**: the 2026 `/items` endpoints (`item` instead of `track` per row, `items` instead of `tracks` in playlist objects); both shapes are accepted.
- **Matching**: implemented as described (ISRC, then title + artist + duration with version notes; likely matches are offered as "Play … from your library? Yes / No", both directions). The MusicBrainz ISRC lookup (step 2) isn't implemented yet.
- **Remote mode**: "Play On…" moves playback to a device, taking the current song along when Spotify has it (the following songs go into the device's queue). The player shows the device's song, as the library song when it matches (cover, lyrics). Starting an album, playlist or song in remote mode plays it on the device the same way; Play Next and Add to Queue go into the device's queue (the API can only add at its end, not remove or reorder). The queue view shows the device's queue, read-only. "Play Here" returns.
- **Search** for a library song: ISRC, then title and artist as fields, as plain words, with the album artist, and the title alone, stopping at a sure match.
- **Editing, for now**: linked playlists take added and removed songs only; reordering and renaming them come later (the outbox already supports both).
- **Others' playlists**: Spotify answers 403 for their entries in Development Mode, even when the list shows a song count. Gatefold asks once per snapshot and then shows them as hidden; reading them needs librespot (phase 8).
- **Searches**: a failed search (offline, too many requests) is never taken as "not on Spotify"; it's retried later. Spotify's waits up to 30 s are waited out.
- **Not yet**: deleting or unfollowing Spotify playlists (the follow endpoints moved to `/me/library`), uploading a changed cover to Spotify, the artist page section "On Spotify, not in your library".

---

## 10. Companion Tools: lyrembed, cdimport & Ear Tag

lyrembed and cdimport are separate Python programs installed with pipx, each with a CLI and a local browser page (127.0.0.1 with a secret token). Ear Tag (Flatpak `app.drey.EarTag`) is the tag editor (§10.3). The player doesn't duplicate what they do. It **starts them**, **opens their pages in the default browser**, and **reads what they write to the files**. A GTK port of the tools isn't planned.

**Contract:** the **command line is the stable interface**: arguments, exit codes, `--version`, and the printed line "`<tool> is running at <url>`". Their page JSON APIs (`/api/library`, `/api/jobs`, …) are internal and are **not** called by the player. The player gets results by reading the files afterwards, which it does anyway through inotify. Where the CLI is missing something, §10.4 lists small additions to the tools themselves.

### 10.0 Shared plumbing (`tools/` module)
- **Detection**: look in `PATH`, then `~/.local/bin` explicitly. Apps started from the GNOME menu often don't have `~/.local/bin` in `PATH`, the same issue the lyrembed README notes for fish. Paths can be overridden in Preferences → *Integrations*.
- **Feature probing**: `lyrembed --version` / `cdimport --version` → enable features by version, as cdimport does with lyrembed. The Integrations page shows *found / version / path* and the install command (`pipx install …`) when a tool is missing. The player never installs them itself.
- **Process control**: `gio::Subprocess` with stdout/stderr pipes read asynchronously on the main loop. To stop a tool, the player sends SIGINT (it behaves as with Ctrl+C, so cdimport finishes cyanrip's log), never SIGKILL.
- **Pages in the browser**:
  - Start the tool with `--no-browser` and read the `… is running at http://127.0.0.1:PORT/?token=…` line from stdout. Open it in the default browser with **`gtk::UriLauncher`**.
  - The player keeps the URL while the tool runs. An `AdwBanner` in the library shows *"lyrembed is open in your browser · Show page · Stop"*, or *"Importing a CD with cdimport · Show page · Stop"*. **Show page** reopens the address if the tab was lost. **Stop** sends SIGINT.
  - No `--return-url` is passed. The pages keep their normal **Quit** button, and the player reacts to the **process exit**. Closing the tab also ends the tools after a short while (lyrembed), or at the next question (cdimport, never during a rip).
  - cdimport's hand-over to lyrembed's page and back (its lyrics step) happens entirely in the browser and needs nothing from the player.
- **Audio conflict**: lyrembed's editor plays the track in the page. When a lyrembed page opens, the player **pauses** and remembers the position. It resumes only if the user presses play (no auto-resume over the tool's audio).
- **Picking up results**: tools write into the music files, and inotify triggers a re-read of exactly those tracks. The lyrics pane reloads live without interrupting playback. Writes replace files atomically (new inode), and GStreamer keeps reading the old file descriptor, so this is safe on Linux.
- **Native app**: the RPM build (§12) starts the tools directly. No `flatpak-spawn --host` or sandbox permissions are needed. Ear Tag, as a Flatpak, is started through `flatpak run` (§10.3).

### 10.1 lyrembed: lyrics search, checking and romanization

| Where in the player | What happens | Command |
|---|---|---|
| Lyrics pane, empty state *"No lyrics"* | **Find automatically**: background job, a spinner in the pane, then a toast with the result | `lyrembed <album dir> --auto [--romanize]` |
| Lyrics pane / track row ⋮ | **Check & edit in lyrembed…**: lyrembed's page for the album in the browser | `lyrembed <album dir> --no-browser` |
| Album panel ⋮ | **Fetch missing lyrics for album**; **Save romanization to files** (§5.1) | `lyrembed <album dir> --auto`; romanizing: the page's bulk action until a CLI option exists (§10.4) |
| In the background | **Automatic romanization** for display (§5.1) | `lyrembed romanize --stdio` (new, §10.4) |
| Library ⋮ (later) | **Fetch missing lyrics for the whole library**: a queue of `--auto` jobs, **one album folder per call** (lyrembed refuses folders over `max_file_count`, 250 by default, and goes at most 3 levels deep), one job at a time, stoppable | per album |

- **Exit codes** are interpreted as follows: `0` done (which can also mean "nothing confident found", so the player re-reads the tracks to see which still have no lyrics, as cdimport does), `1` some files failed, `2` unusable path or config, `130` cancelled. Tracks still without lyrics after `--auto` get a *"Choose manually in lyrembed…"* action.
- **The romanization layout matters for dual-line display**: lyrembed's default `romanization_layout` is `"replace"`, which drops the original script. Preferences → Integrations shows a hint if lyrembed's config (`~/.config/lyrembed/config.json`) doesn't say `"below"`. The player only reads that file and never writes it.
- **Artist aliases**: lyrembed has about 205 alias groups (K-pop, J-pop, C-pop, spellings with and without diacritics) plus the user's own. The player reads them through `lyrembed --list-aliases` (§10.4 proposes JSON output) and caches them. It uses them for:
  - search ("Hoshino Gen" finds 星野源),
  - artist merging (§3.3 rule 4),
  - showing the Latin name as a subtitle under a native-script artist name on the artist page.
- **Romanization**: automatic, with lyrembed's engine. See §5.1.

### 10.2 cdimport: getting CDs into the library

| Where in the player | What happens | Command |
|---|---|---|
| Library main menu → **Import CD…** | Opens cdimport's page in the browser, plus a status banner in the player | `cdimport --web --no-browser -l <library folder>` |
| Audio CD inserted (GIO `GVolumeMonitor`, content type `x-content/audio-cdda`) | `AdwBanner`: *"Audio CD detected — Import"* | as above |
| Unfinished imports | Banner: *"1 unfinished CD import — Continue"*. The player finds `.cdimport.json` files in cdimport's `work_dir` (read from `~/.config/cdimport/config.json`) and only checks whether they exist, never their contents | `cdimport <album dir> --web --no-browser …` |
| After the import | cdimport moves the album to `<library>/<albumartist>/<folder>`. inotify sees the new folder, and the player shows a toast *"New album: \<title\> — Play / Show"* | — |

- **Library folder agreement**: on first run, if cdimport is installed, the player reads its `library_dir` and offers to add it as a library folder. When starting cdimport, the player passes `-l` with its own primary folder, so imports always land where the player looks.
- **Multi-disc merges**: cdimport can add disc 2 into disc 1's existing folder. The scanner treats new files in a known album folder as part of that album (same release MBID / album + album artist), not as a new album.
- **Debounce**: an album appears as a folder move (atomic on one filesystem), but cover and cue files can follow shortly after. The scanner waits ~2 s after the last event in a folder before reading it.
- **Covers**: cdimport writes `cover.jpg` and an embedded front cover, which the cover pipeline (§4.3) uses without extra work.
- cdimport already chains lyrembed for the lyrics step, so the player adds nothing there.

### 10.3 Ear Tag: tag and cover editing
The player never writes tags itself (Q8). Every editing action opens the files in **Ear Tag**, GNOME's tag editor (installed here as Flatpak `app.drey.EarTag`). It's the same editor cdimport uses by default.

| Where in the player | Action | Files passed |
|---|---|---|
| Album panel ⋮ | **Edit tags in Ear Tag…** | all tracks of the album (of the chosen edition) |
| Track row ⋮ (expanded song details) | **Edit tags…** | that track |
| Album panel cover ⋮ | **Change cover…** (Ear Tag edits embedded covers) | all tracks of the album |
| Artist page ⋮ | **Edit album artist tags…** (e.g. to set `ALBUMARTISTS` for a real split, §3.3) | all tracks of that artist's albums |
| Lyrics pane ⋮ | — (lyrics stay with lyrembed, §10.1) | — |

- **Command**: `flatpak run --file-forwarding app.drey.EarTag @@ <files> @@`, the same template cdimport uses. File forwarding hands the files over through the document portal, so Ear Tag can write them even outside its sandbox permissions.
- **Detection**: `flatpak info app.drey.EarTag` (user or system installation). If it's missing, the menu entries show *"Install Ear Tag…"* with a link to `appstream://app.drey.EarTag`, which opens GNOME Software.
- **Other editors**: Preferences → Integrations → *Tag editor* offers Ear Tag (default), Kid3 and MusicBrainz Picard, plus a custom command. These use the same command-template format as cdimport's `editors` config (`{files}`, `{dir}`). If cdimport's config names an editor, that's preselected, so both programs use the same one. Picard is the fallback for fields Ear Tag may not show, such as multi-valued `ALBUMARTISTS`. Verify this with the installed Ear Tag version.
- **No waiting**: Ear Tag returns at once, especially when its window is already open, so the player doesn't wait for the process. Saved changes arrive through inotify, and the affected tracks/albums are re-read and re-grouped (album artist, editions, release type). An album can move to another artist live.
- **During playback**: same as lyrembed's writes (§10.0). The current track keeps playing, and the new metadata shows when the re-read finishes.

### 10.4 Proposed additions to the tools

These are small changes in lyrembed and cdimport that would make the integration cleaner. **The first one is required for automatic romanization (§5.1).** The others are optional: the player works without them, with fewer features.

| Tool | Addition | Lets the player… |
|---|---|---|
| lyrembed | **Required:** `lyrembed romanize --stdio`, a long-running filter. Input: JSON lines `{"id":…, "lyrics":"…", "han_hint":"ja"\|"zh"\|null}`. Output: `{"id":…, "lyrics":"…" (layout below), "scripts":["hangul",…], "unresolved_han":n}`. Writes nothing, no network. | romanize automatically for display (§5.1) with lyrembed's engine, with ~1 s start-up once instead of per song |
| lyrembed | `--auto --romanize-only` (romanize the embedded lyrics of a folder, layout below, with backups, no search) | offer **"Save romanization to files"** without opening the page |
| lyrembed | `path` may be a **single audio file**: the page opens directly on that track | open "Check & edit" for the track that's playing, not the whole album |
| lyrembed | `--json` for `--auto`: one JSON line per track (`{"file":…, "result":"embedded/none/instrumental/failed", "source":…}`) | show per-track progress and results instead of re-reading files |
| lyrembed | `--romanization-layout {replace,below}` for one run | always get dual-line output without touching the user's config |
| lyrembed | `--list-aliases --json` | read alias groups without parsing human-readable text |
| cdimport | Machine-readable final line, e.g. `cdimport imported <path>` | open the imported album directly instead of guessing from inotify |
| cdimport | `MimeType=x-content/audio-cdda;` in its desktop entry | GNOME's own "on CD insert" choice offers cdimport, even with the player closed |
| cdimport-cover | *(not needed: covers are changed in Ear Tag, §10.3)* | — |

---

## 11. Desktop Integration
- **MPRIS** (`mpris-server`): metadata, cover art URL (file:// to cached cover), position, shuffle/loop status. This gives GNOME Shell media controls and media keys.
- **Notifications** on track change (optional, off by default since Shell already shows media controls).
- **Background portal** for keep-playing-when-closed.
- **Keyboard shortcuts** (`AdwShortcutsDialog`): Space play/pause, Ctrl+←/→ prev/next, Ctrl+F search, Ctrl+L lyrics, Ctrl+U queue, Ctrl+1/2/3 views. Ctrl+Q closes the window (and quits), as in other GNOME apps.
- **Drag & drop** of files/folders onto the window → play now / add to queue.
- **Accessibility**: every cover has accessible labels, and lyrics are readable by Orca.

---

## 12. Build & Packaging (RPM, local)
For now the app is built as an **RPM for Fedora 45 and installed locally**. There's no Flatpak and no distribution. COPR stays an easy step later, because the same spec file works there.

- **Build system**: Meson + Cargo (GNOME Rust app layout). The spec uses the standard `%meson` / `%meson_build` / `%meson_install` macros.
- **Rust dependencies are vendored**: `cargo vendor` produces a second source tarball (`Source1`), and the build runs offline with `--offline`/`--frozen`. Many crates used here (lofty, bliss-audio, mpris-server, oo7, librespot) aren't packaged in Fedora, so vendoring is the practical way. `Cargo.lock` is committed.
- **Clean builds** with `mock -r fedora-45-x86_64` (catches missing BuildRequires), quick local builds with `rpmbuild -ba`. A `make rpm`/`just rpm` target wraps both.
- **Build conditions**: `%bcond spotify_stream 0` → `rpmbuild --with spotify_stream` enables the librespot Cargo feature (§9.4). The default build contains no librespot code.

| | Packages (Fedora 45) |
|---|---|
| **BuildRequires** | `meson`, `cargo`, `rust`, `blueprint-compiler`, `gtk4-devel`, `libadwaita-devel`, `gstreamer1-devel`, `gstreamer1-plugins-base-devel`, `sqlite-devel` (or bundled SQLite via rusqlite), `desktop-file-utils`, `libappstream-glib`, `gettext`. Plus the FFmpeg dev libraries from `ffmpeg-free-devel` if bliss uses its FFmpeg decoder, see below. |
| **Requires** | `gtk4`, `libadwaita`, `glycin-loaders`, `gstreamer1-plugins-base` (`playbin3`), `gstreamer1-plugins-good` (FLAC, `rgvolume` ReplayGain), `gstreamer1-plugins-bad-free` (`bpmdetect`/SoundTouch), `gstreamer1-plugin-libav` (AAC/ALAC in the M4A files) |
| **Recommends** | `gstreamer1-plugins-ugly-free` (WMA via `asfdemux`, not needed for `~/Musik` today), `flatpak` (for Ear Tag) |
| **Not RPM dependencies** | lyrembed, cdimport (pipx), Ear Tag (Flatpak): detected at runtime (§10) |

- **Installed files**: `/usr/bin/gatefold`, the desktop file and AppStream metainfo (validated in `%check`), the GSettings schema, icons (scalable + symbolic), the GResource bundle, and translations.
- **bliss decoding** (verify in an early spike): bliss-audio decodes through FFmpeg by default. On Fedora that means `ffmpeg-free`, which has to decode the library's FLAC, AAC/ALAC (M4A) and MP3 files. The alternative is feeding bliss samples decoded by GStreamer, so there's one decoding stack, and `bpmdetect` can run in the same pipeline pass. That's preferred if the bliss version supports external samples.
- **Name and IDs**: the app is called **Gatefold**.

  | What | Value |
  |---|---|
  | App ID (desktop file, metainfo, GSettings schema, D-Bus name, icons) | `io.github.sobakah.Gatefold` |
  | MPRIS bus name | `org.mpris.MediaPlayer2.Gatefold` |
  | Command / binary / RPM package | `gatefold` (spec file `gatefold.spec`) |
  | Data (DB, portraits, analysis) | `~/.local/share/io.github.sobakah.Gatefold/` |
  | Cache (cover thumbnails) | `~/.cache/io.github.sobakah.Gatefold/` |
  | Keyring entries (Spotify tokens) | attribute `application = io.github.sobakah.Gatefold` |
  | Rust crates | `gatefold` (app), `gatefold-core`, `gatefold-metadata`, `gatefold-lyrics`, `gatefold-similarity`, `gatefold-tools`, `gatefold-spotify` |
  | User-Agent for MusicBrainz/Wikidata/fanart.tv | `Gatefold/<version> ( https://github.com/sobakah/gatefold )` |

---

## 13. Roadmap

| Phase | Scope |
|---|---|
| **0: Skeleton** | Meson/Cargo template, RPM spec + local `mock` build, Blueprint, GSettings, empty Library + bottom-sheet Player |
| **1: MVP** | Scanner + SQLite (symlinks refused, edition subtitles + merging, release types), Albums grid + side panel + expanding song rows, GStreamer gapless playback, ReplayGain, basic queue + **persistence**, repeat, plain shuffle, MPRIS |
| **2: Artists** | Album-artist model (tag-trusting, case/alias identity, collaboration links), artist grid + page, MusicBrainz/Wikidata/fanart.tv lookups, portrait customization, Wikipedia/MusicBrainz/Spotify **links** |
| **3: Player polish** | Cover-color theming, adaptive layouts, lyrics (embedded incl. LRC-in-USLT, dual-line), queue sections + drag & drop |
| **4: Companion tools** | `tools/` module, Integrations preferences, Ear Tag (edit tags/cover, live re-read), lyrembed (auto fetch, page in browser, aliases, **automatic romanization** once `romanize --stdio` exists), cdimport (Import CD, CD-insert banner, unfinished imports, new-album toast) |
| **5: Playlists** | Local playlists (data model already mixed-ready, §9.5), XSPF/M3U8 import/export, mosaic covers, search (FTS5) |
| **6: Smart shuffle** | Analysis worker (bliss + `bpmdetect`), genre/BPM/artist/sound mixing, "keep playing similar" |
| **7: Spotify account** | OAuth PKCE, device picker + remote mode, **Spotify playlists** with ISRC/fuzzy **local matching** (matched entries already play locally, the rest show as "Spotify only"), **two-way editing** of owned playlists with outbox sync, "Create on Spotify" |
| **8: Spotify stream** | librespot → appsrc sink, Spotify-only entries playable, mixed queue/playlists, loudness alignment, Connect receiver (feature-gated) |

The tool additions in §10.4 can be done in lyrembed/cdimport at any time. Phase 4 works without them, except automatic romanization.

### Optional future features (not scheduled)
Ideas that came up along the way, to pick from once the roadmap is done:
- **Lyrics for Spotify-only songs** from LRCLIB (free public lyrics database, by title, artist and length): shown only, never embedded. In remote mode the Lyrics pane is off for such songs until then (§9.7).
- **Reordering and renaming Spotify playlists** from Gatefold. The outbox supports both, but they're switched off until the rest of the app is stable (§9.7).
- **MusicBrainz ISRC lookup** for files without an ISRC, to match Spotify entries more reliably (§9.5, step 2).
- **Unfollowing or deleting Spotify playlists** (`/me/library`) and **uploading a changed cover** to Spotify.
- **"On Spotify, not in your library"** on the artist page (§9.3).
- **ListenBrainz similar artists** for smart shuffle's "same artist" criterion (§7).

---

## 14. Open Questions

### Answered
- **Q1** Joint album artists: confirmed. Trust the tag, link without splitting, fix real splits in the tags (§3.3).
- **Q3** Romanization: automatic, with lyrembed's engine, display-first and optionally saved to the files (§5.1).
- **Q4** Audio analysis: yes (§7).
- **Q6** Spotify: mixed playlists, Spotify playlists playable, local copies preferred (§9.5).
- **Q10/Q11** Tool pages open in the browser. No GTK port of the tools.
- **Q12** Symlinks are not allowed for now. Only `~/Musik`, and the rclone Google Drive copy stays out (§3.1).
- **Q13** Spotify playlists are editable two-way where Spotify allows it (owned/collaborative, §9.6).
- **Q14** Dropped: without the Google Drive copy, duplicates only come from merged editions and singles vs. albums (§9.5 rule 4).
- **Editions**: subtitle in the album panel only, same-tracklist editions merged (§3.4).
- **Q2** "Appears on": yes, collapsed, only for artists with their own album-artist entry (§4.2).
- **Q5** Grids: library tiles show cover, album, artist. Artist page tiles show cover, album. No size slider (§4.1, §4.2).
- **Q7** Scrobbling: no.
- **Q8** No app-only metadata overrides (except portraits and Spotify match choices). Tag and cover editing goes to Ear Tag (§10.3).
- **Q9** Distribution: none for now. Local RPM build for Fedora 45 (§12).
- **Q15** Name: **Gatefold**, app ID `io.github.sobakah.Gatefold` (§12).

### Still open
None at the moment.
