# Gatefold: Concept Draft

*Status: draft v0.3 (2026-11-05). Target: Fedora 45 (GNOME 51, GTK 4.24, libadwaita 1.10, GStreamer 1.28).*

### Decisions so far
| Topic | Decision |
|---|---|
| Name | **Gatefold**, app ID `io.github.sobakah.Gatefold`, command `gatefold` (§12) |
| Language / library | Rust; own scanner + SQLite |
| Album details | Side panel (a sliding panel like `AdwOverlaySplitView`); only artists get their own page |
| Metadata source | **Local tags are the source of truth.** Online data only adds what tags can't hold (portraits, links, bios). |
| Spotify | Links, remote control, streaming (librespot), **Spotify playlists with local substitution**, mixed playlists, **two-way editing** of owned Spotify playlists (§9.6) |
| lyrembed / cdport | Started as external programs; their pages open **in the default browser**; no GTK port planned |
| Romanization | **Automatic**, using lyrembed's engine (§5.1) |
| Audio analysis | Wanted (bliss + BPM detection, §7) |
| Album editions | Not part of the title. Shown only as a subtitle in the album panel; same-tracklist editions merged (§3.4) |
| Grids | Library album grid: cover, album, artist. Artist page: cover, album (§4.1, §4.2) |
| "Appears on" | Yes, collapsed, only for artists with their own album-artist entry (§4.2) |
| Tag editing | Not in the player; delegated to **cdport's tag editor** by default, Ear Tag and others selectable in the RPM (§10.3) |
| Scrobbling | No |
| Packaging | RPM on GitHub releases (§12); Flatpak and distribution are phase 9 (§13) |
| Symlinks | Not allowed for now (§3.1). The rclone-synced Google Drive copy is not part of the library. |
| Joint album artists | Trust the tag, link without splitting (§3.3) |

## 1. Vision

A local-first music player for people who listen to **albums**, follow **artists**, and curate **playlists**.

- **Sleek by default.** The first thing you see is cover art and very little chrome.
- **Expandable.** Lyrics, the queue and details can be opened when you want them, and they stay out of the way when you don't.
- **Cover art first.** Covers are the main navigation element and the main visual of the player.
- **Clean artist list.** Only album artists get entries. Featured or collaborating track artists don't create clutter.
- **Built, not reinvented.** Every subsystem uses an established, maintained library or protocol.

Non-goals for v1: tag editing inside the player (delegated to cdport's editor or Ear Tag), online lyrics, scrobbling, podcasts, a "Songs" mega-list as a primary view, Flatpak/Flathub distribution, and Windows/macOS support.

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
| Metadata | **musicbrainz_rs**, Wikidata REST/SPARQL, Cover Art Archive, Deezer, Spotify | Free, open and ID-based, with no scraping. |
| Audio similarity | **bliss-audio** (bliss-rs) | Local song analysis (tempo, timbre, chroma, loudness) that gives a distance metric. Spotify's audio-features API is no longer available, so this replaces it. |
| BPM detection | GStreamer **`bpmdetect`** (SoundTouch, installed on Fedora 45) | The reference library has **no BPM tags at all**, so BPM has to be measured. This runs in the same analysis pass as bliss. |
| Desktop integration | **mpris-server** crate | Media keys, GNOME Shell media controls, KDE Connect. |
| Secrets | **oo7** (GNOME's Rust Secret Service client) | Stores Spotify tokens in the keyring; without a keyring service, in a file only the user can read (§11). |
| Settings | **GSettings** | Standard GNOME settings storage, available through gio. |
| Spotify streaming | **librespot** (optional Cargo feature) | The only maintained open-source Spotify client library. See §9. |
| Companion tools | **gio::Subprocess** (async, GLib-integrated) | Runs lyrembed, cdport and Ear Tag as separate processes. The tools' pages open in the default browser. See §10. |
| Packaging | **RPM** for Fedora 45, built locally (`rpmbuild` / `mock`) | Personal use only for now. Native packaging keeps direct file access and lets the app start lyrembed, cdimport and Ear Tag without sandbox workarounds. See §12. |

### Crate / module layout (Cargo workspace)

```
gatefold/           # this repository (Cargo workspace root)
├── app/            # GTK UI: windows, widgets, Blueprint files, GObject wrappers
├── core/           # no GTK dependency: library DB, scanner, queue, playback engine
├── metadata/       # MusicBrainz, Wikidata, Deezer, Cover Art Archive providers
├── lyrics/         # LRC/embedded parsing, dual-line pairing
├── similarity/     # bliss analysis worker + "similar track" queries
├── tools/          # lyrembed / cdport: detection, version probing, process control
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
   4. To really split an entry, fix the tag (set `ALBUMARTISTS`). The artist page offers *"Edit album artist tags…"*, which opens that artist's albums in the configured tag editor (cdport's by default, one album at a time, §10.3).
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

- **Albums**: `GtkGridView` of covers. Each tile shows **only cover, album title and album artist** (one line each, ellipsized). The cover size is set in Preferences → Library → Appearance (120–280 px, default 160) and stays the same when the window or a panel changes width: only the number of columns and the room between them change (in the album, artist and playlist grids and on artist pages). Only a window too narrow for two columns makes covers smaller. Sortable by artist, title, year or date added, with filters (release type, genre, soundtracks, compilations). Clicking a cover opens the **detail side panel** (sidebar on the end side). Nothing is pushed as a new page.
- **Side panels** (album, playlist, artist page) borrow the player's look: the cover (the portrait on artist pages), blurred and tinted with its dominant color, behind the top of the panel, fading into the normal background before the songs. It's the player's backdrop exactly: the same blur (60 px), strength (35 %) and tint, and the cover as large as the window (as the player shows it), centered on the panel, so it reads as color, not as a second copy of the picture in front of it. On the wide artist page it's a lower band (380 px).
- Side panels have no close button: a grip on their left edge, halfway down (the player's grip, upright), closes them. The player's grip on top is clicked the same way to hide the player (and can still be dragged down, or closed with Escape or a click beside it); it has no arrow besides (removed November 2026, it did the same). All grips share one look and grow a little under the pointer.
- **Sliding panels** (November 2026): every side panel (album and playlist panels, the artist page, the album panel on an artist page) slides like a record sleeve pushed in, in one widget (`SlideView`) so they all move alike: a spring a little slower than libadwaita's that runs about 2 % past its place and settles back (drawn a little wider meanwhile, not laid out again), and slides out without a bounce; a soft shadow on its leading edge. Beside the content or over it (narrow windows, artist pages), where the content is dimmed and a click on it or Escape closes the panel. The content takes its final width once, not on every frame: as the panel starts opening, and as it starts closing (under the panel, so no empty strip shows behind it). The player's sheet still slides with libadwaita's own animation, which can't be changed.
- On an artist page an album's panel covers the page's whole height, from the top bar down, as in the library.
- **Album detail panel**: large cover, title, edition subtitle (§3.4, plus the list of owned versions when editions were merged), album artist(s) as links (→ artist page), year, length, actions (Play, Shuffle, Add to queue, Play next, Add to playlist). Below that is the tracklist, grouped by disc.
- **Songs expand in place**: clicking a track row's chevron opens a `GtkRevealer` under the row with track artists/featuring, composer, genre, BPM, format/bitrate/sample rate, a lyrics-available indicator with a short excerpt, and actions (Play next, Add to queue, Add to playlist, Go to artist). Double-click or Enter plays the track.
- **Narrow windows** (`AdwBreakpoint` < ~700 px): the side panel turns into an overlay sheet that covers the grid, and the mini bar stays visible.
- **Artists**: grid of circular portraits (`AdwAvatar`-style, with the actual image) and names, using only album artists. Clicking one pushes the **artist page** onto an `AdwNavigationView`. This is the **only** real page navigation in the app.
- **Playlists**: grid of playlist covers (custom image or an automatic 2×2 cover mosaic). Clicking opens the same detail side panel with an editable track order (drag & drop, remove, rename).
  - Like on Spotify, a playlist can be listed by its own order, title, artist, album, date added or duration, each also reversed. Until one is chosen for a playlist, the default from Preferences → Library → Playlists applies, one for Gatefold's playlists and one for Spotify's (custom order by default; "Use Default Sort" in the menu goes back to it). The choice is kept in the existing columns, so older versions still open the database: `sort_by` 'custom' and not reversed means "not chosen", a chosen custom order is stored as 'own'. A chosen sort is remembered per playlist and only changes the view (and what Play plays), not the stored order; dragging only works in the playlist's own order. The date added is Spotify's for Spotify entries, else when the song was added (unknown for entries from before 0.1.1).
  - **Search in a playlist** (November 2026): the field above the songs searches the playlist's own songs (title, artists, album; every word must match), showing "3 of 50 songs". Its **+** button switches it to adding: then it finds songs in the library, and with the Spotify button beside it also Spotify's songs that aren't in the library (after the library's, marked with Spotify's icon; one search request a moment after typing stops). That Spotify search is off by default in Gatefold's playlists and on in Spotify's; each playlist opens with its default. A new, empty playlist opens in adding. Arranging a Spotify playlist's order shows all songs again.
  - Its songs have the same actions as an album's: Add to queue on the row, Play next, Add to playlist, Go to album and Go to artist in its menu (Spotify songs too: their pages are looked up on Spotify). The playlist's menu has Play next, Add to queue and Add to playlist for all of it.
- **Search**: a single search field (`GtkSearchBar`) backed by SQLite FTS5. Results are grouped into Artists / Albums / Songs / Playlists in a popover-like card that floats over the current view. The field takes typing as soon as search opens; Escape, the search button, a click beside the card or opening a result closes it.

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
- The page covers three quarters of the window (all of it below 600 px), with the grid visible beside it. Like the other side panels, it's closed with the grip on its left edge; it slides back out to the right.
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
│                   ━━━                  ⋮     │
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

The artist and album under the title open their pages: the library's for library songs; for Spotify songs (streamed, or on a Spotify device) the song is looked up on Spotify, and the artist's page is the library's when the artist is there.

A + button left of the title (the volume sits right) is "Add to Playlist…" for the song that plays, as is the Now Playing row's right-click menu in the queue; its tooltip names the playlists that already have it. Song Info lists them too. A song shown from a Spotify device is first kept as a song to stream (`stream_track`), as its stand-in ID is only a hash.

**Which playlists have a song:** the playlists with an entry for it (`local_track_id`), or for its Spotify URI. A library song's URI counts only for sure matches, and a streamed song also finds the library song that surely matches it. "Add to Playlist…" lists those playlists first with a check mark ("Has this song"); choosing one again removes the song's entries there (one Spotify removal for all). For several songs, a playlist with only some of them ("Has 3 of these songs") gets the missing ones.

Expanded: the **Lyrics** and **Queue** toggles open side panes. Whether they're open when Gatefold starts is a preference each (as last left, shown, hidden). On wide windows the cover moves left and the pane sits next to it (`AdwMultiLayoutView` switching between "cover only", "cover + lyrics", "cover + queue" and "cover + lyrics + queue" layouts depending on width). On narrow windows the pane replaces the cover and a small cover stays in the header.

- Accent color: the dominant cover color is applied through a scoped CSS provider (`@accent_bg_color`, background gradient) **only inside the player view**, with contrast checked against WCAG AA. The rest of the app follows the system accent.
- Cover sources, in priority order: embedded picture (front) → `cover.*`/`folder.*`/`front.*` in the album folder → (optional, opt-in) Cover Art Archive by release MBID. Cached thumbnails go in `~/.cache/io.github.sobakah.Gatefold/covers/` at 3 sizes (grid, panel, player), decoded with glycin.

- **Visualizer** (November 2026): 32 bars between the cover and the title in the player; in the mini player, bars in the space between the song and the controls, as many as fit (4 to 24, 7 px each) and taller the more there are, in the cover color, faint at rest and full when loud; they rise at once and fall back gently. A `spectrum` element ends both audio filters (with ReplayGain behind `rglimiter`, without it alone), so it shows the levelled music, not the volume slider; it posts only while a visualizer is on screen, 25 frames a second of 1024 bands. Frames wait until the sink plays them (their running time against the pipeline's and its latency), so the bars move with what's heard; after a seek or a new song the queue starts over. Bars are log-spaced from 50 Hz to 16 kHz, tilted against music's falling highs and scaled for ReplayGain'd music (measured on the reference library). Crossfades show the song fading in. A still, faint strip for Spotify songs (Soloist plays them itself; tapping its PipeWire output was left out as fragile), while paused and when nothing plays; then it stops drawing. Preferences → Playback → *Visualizer* (`visualizer`, on); off while GNOME's animations are off.

---

## 5. Lyrics

**Sources (offline only for v1, priority order):**
1. Sidecar `.lrc` next to the audio file (`Song.lrc`). Sidecars win because they're easier to fix.
2. Embedded synced lyrics: ID3 `SYLT`, ID3 `USLT` containing LRC timestamps, Vorbis `LYRICS`/`SYNCEDLYRICS` containing LRC, MP4 `©lyr` containing LRC. Each container is checked for LRC timestamps before being treated as plain text. This matters because **lyrembed writes synced lyrics as LRC into `USLT`/`LYRICS`/`©lyr`** and removes `SYLT` when it writes.
3. Embedded/sidecar unsynced: ID3 `USLT`, plain `LYRICS`, `.txt` sidecar → shown as static, scrollable text.

A small tag in the pane's corner says where the shown lyrics come from: *In the File* (tags), *LRC File*, *Text File*, or *LRCLIB* (Spotify songs, §9).

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
2. Those lyrics are sent to lyrembed's D-Bus service (`Romanize`, lyrembed 1.2.0, §10.0) or, without it, a long-running **`lyrembed romanize --stdio`** helper (JSON lines in, JSON lines out, writes nothing). This is an addition to lyrembed (§10.4), and it's **required** for this feature. The process is started on first need and stopped after 5 min idle, so Python/pykakasi start-up (~1 s) happens once, not per song.
3. The result is stored in the player's DB, keyed by a **hash of the lyrics text**. If the file's lyrics change (e.g. re-fetched with lyrembed), the hash changes and romanization runs again.
4. The lyrics pane shows auto-romanized lines exactly like tagged ones, with a small "auto" marker in the pane's header menu (not per line).

**Ambiguous Han characters** (valid as Chinese and Japanese, e.g. `東京物語`): lyrembed resolves about 4 in 5 runs by itself. For the rest the player passes a hint, from local metadata only, in this order:
- Kana elsewhere in the lyrics → Japanese
- `LANGUAGE` tag (`jpn`/`zho`)
- the album's other tracks' lyrics
- the setting *"Ambiguous Han characters: Chinese / Japanese"*, default Japanese (the library's Han-script music is mostly Japanese, e.g. 下村陽子)

**Making it permanent** (optional, one click per album): *"Save romanization to files"* runs lyrembed on the album (its bulk action *Romanize embedded lyrics* with layout `below`). This writes the dual-line format into the tags with lyrembed's backups, so other players (Navidrome, Jellyfin, Symfonium) see it too. Until then the files stay untouched.

**Fallback without lyrembed** (not installed, or older than the version with `romanize --stdio`): no auto-romanization. The Integrations page says why and shows the install/update command.

**As built (November 2026, lyrembed 1.1.1):** for the song that plays (local lyrics without romanization, and LRCLIB lyrics for Spotify songs), not prefetched; the lyrics show at once and the romanization replaces them when it's there; not with the "original only" layout. One helper process, stopped after 5 minutes idle and when Gatefold quits; results (also "nothing to do") cached by a hash of the text and hint (migration 025). The Han hint comes only from the file's language tag (`TLAN`/`LANGUAGE`); no setting yet, and no "auto" marker. In lyrembed the hint only settles Han runs the lyrics leave open; Chinese evidence still wins.

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
- **Recently Played** sits above *Now playing* (the last 30 songs; the database keeps 200). It has library songs and Spotify songs that played in Gatefold (streamed: `-stream_track.id`), each with a menu (button and right-click): *Play next*, *Add to queue*, *Add to playlist…*. A click plays the song now. Since migration 026, `play_history` has no foreign key on `track`; a trigger removes deleted library songs. While a Spotify device plays, the section is hidden.
- **Persistence:** queue and state are written to SQLite (debounced, about every 2 s and on shutdown). On start, the queue is restored **paused** at the previous position. Missing files are skipped with a toast. Restoring can be disabled in Preferences.
- Background playback: closing the window while playing can keep playback going (`GApplication` hold + GNOME's Background portal). This is a setting that defaults to *off* (closing quits, and the queue is saved).

### 6.3 Listening statistics (November 2026)
- **What it shows**: main menu → *Listening Statistics* (an `AdwDialog`). A switcher picks the last 7 days, 30 days, 12 months or everything logged (`statistics-period`). The page has the totals (time listened, plays, songs, artists), the listening time as bars (per day; per month for a year and for "All Time" up to three years, per year beyond that; each bar's tooltip names it), then top artists (portraits), top albums (covers), top songs (20, with a Play button that makes them the queue) and top genres (bars of their share). Every entry shows its plays and time, and how it moved against the period before as a badge: ↑3, ↓2 or *New* (none for "All Time"). Artists and albums in the library open their page or panel; a song plays when clicked.
- **The log**: `play_log` (migration 027), never trimmed and not touched by clearing Recently Played. A song counts under the history's rule (half of it, or 30 s) with how far it played. What played is copied in (title, artist, album, album artist, first genre), so the statistics survive rescans and removed songs; library songs and streamed Spotify songs (`-stream_track.id`) alike, devices' songs through their streamed ID. The 200 history entries started it (as whole songs).
- **Counting**: songs by title and artist (the same song from the library and from Spotify counts once); artists by every name in the credit ("RM (feat. Youjeen)" counts for both, `track_artist_names`); albums by title and album artist; genres by the first genre. Ties go by listening time.
- **Privacy**: Preferences → Playback → *Listening Statistics*: a switch (`listening-stats`, on) and *Delete Statistics…* (asks first; Recently Played stays). Nothing leaves the computer.
- Later, maybe: a custom from–to period, importing Spotify's "Extended streaming history" (the JSON from Spotify's privacy download) to fill the past.

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
- **Related artists** (November 2026): ListenBrainz's public similar-artists dataset (labs API `similar-artists/json`, by MBID, read-only, no account, no scrobbling) widens "same artist": a related artist scores up to 0.6 of the same artist, by ListenBrainz's score over the artist's best. Asked only with online lookups and "same artist" on, when smart shuffle needs it: one artist at a time through the metadata worker, at most one request a second (ListenBrainz's rule), each again after 90 days (migration 024), an hour's pause after a 429 or 5xx. Spotify's related-artists and recommendations endpoints are **not available** to new apps since Nov 2024, so they aren't an option.

---

## 8. Artist Portraits & Online Metadata

**Lookup chain**, run lazily when an artist appears on screen, rate-limited to 1 req/s for MusicBrainz, with a descriptive User-Agent and results cached in the DB:
1. **Local**: `artist.jpg`/`folder.jpg` in the artist directory (Kodi/Jellyfin convention).
2. **User override** (always wins once set).
3. **Spotify Get Artist** image, while Spotify is connected (§9): by the Spotify ID from Wikidata or of a followed artist, else the artist search result of exactly that name. Covers nearly every artist, K-pop included, at 640 px.
4. **Deezer** artist picture (no account or key; 1000 px): the search results of exactly that name; of several, the one whose albums share the most titles with the library's (else one with ten times the fans of the others, else none).
5. **Wikidata `P18`** → Wikimedia Commons image (freely licensed; attribution shown in a tooltip).
6. Fallback: generated initials avatar tinted with the color of their most common album cover.

fanart.tv was dropped in 0.1.1: it needed a key and, being volunteer-made, rarely had more than Commons for this library. Lookups done before Spotify and Deezer were sources are repeated once (database migration 019).

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
- **Since Feb/Mar 2026**, Web API apps in Development Mode require the **app owner to have Spotify Premium**, are limited to **5 authorized users**, and have fewer endpoints. `/search` is capped at 10 results, and answers `403 Insufficient client scope` when it's given `market=from_token` (playlist reads still accept that), so searches leave the market out. Several batch endpoints, `/artists/{id}/top-tracks` and others were removed. Extended Quota Mode, which has no such limits, is effectively only open to large organizations.
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
- **Matching**: implemented as described (ISRC, then title + artist + duration with version notes; likely matches are offered as "Play … from your library? Yes / No", both directions).
- **MusicBrainz ISRCs** (step 2, November 2026): by release rather than recording (the reference library has release IDs, no recording IDs): one request per album (`release/{id}?inc=recordings+isrcs`), matched by disc and track number, kept in `mb_isrc` (migration 020), never written to the files. With online lookups on, after a Spotify sync; each album once, again after half a year. Used for matching and for the `isrc:` search; entries are matched again afterwards. The test library: 242 of 273 songs without ISRC got one, in 30 requests.
- **Remote mode**: "Play On…" moves playback to a device, taking the current song along when Spotify has it (the following songs go into the device's queue). The player shows the device's song, as the library song when it matches (cover, lyrics). Starting an album, playlist or song in remote mode plays it on the device the same way; Play Next and Add to Queue go into the device's queue (the API can only add at its end, not remove or reorder). The queue view shows the device's queue, read-only. "Play Here" returns.
- **Search** for a library song: ISRC, then title and artist as fields, as plain words, with the album artist, and the title alone, stopping at a sure match.
- **Editing**: linked playlists take added and removed songs from anywhere. Reordering and renaming are deliberately harder to reach than for local playlists, so a Spotify playlist isn't changed by accident: "Change Order on Spotify" in the menu makes the songs draggable until Done (the moves wait until then and go out together), and "Rename on Spotify…" asks for the name (clicking the name doesn't rename linked playlists). "Change Cover…" on an own playlist uploads the cover too (a JPEG of at most 640 px and 190 KB; mirrors have no "Use Automatic Cover", Spotify can't go back to its mosaic).
- **Sending changes** (November 2026): edits wait 4 s after the last one. While a playlist's changes are on their way, it can't be edited ("sending changes to Spotify…") until Spotify's answer and the compare re-read are in. Adds and moves also name the song they follow: when the playlist changed elsewhere since the last read (its snapshot differs), Spotify's entries are read and the changes placed by that song. An addition whose answer got lost (timeout, reset, 5xx) is marked and checked before it's sent again. Playlists with changes waiting aren't re-read (that would undo them in the mirror). "Create on Spotify" runs as a sync job, once per playlist; a playlist created in the session isn't unlinked while Spotify's list doesn't show it yet.
- **Covers**: Spotify playlists show Spotify's image (its own, or its mosaic of the whole playlist), also when some of their songs are in the library.
- **Others' playlists**: Spotify answers 403 for their entries in Development Mode, even when the list shows a song count. Gatefold asks once per snapshot and then shows them as hidden; reading them needs librespot (phase 8).
- **Searches**: a failed search (offline, too many requests) is never taken as "not on Spotify"; it's retried later. Spotify's waits up to 30 s are waited out.
- **Remove from Spotify…** (November 2026): deletes your own playlist on Spotify, unfollows someone else's (`DELETE /me/library?uris=spotify:playlist:<id>`; new logins also ask for `user-library-modify`, an older login that Spotify refuses is asked to connect again). The dialog says Spotify keeps deleted playlists restorable for 90 days (spotify.com/account/recover-playlists) and that followers keep theirs. A mirror can stay as a local playlist (default for your own); a published one always stays. A sync job; changes still waiting are dropped; a removed playlist that Spotify's list still shows for a moment isn't mirrored again.
- **Picking in the Spotify app**: while Gatefold plays through Soloist, a song chosen or skipped to in the Spotify app with more than 15 s of Gatefold's song left is the app's choice: Gatefold lets go and shows it as the receiver. At the end of a song (Soloist's autoplay), Gatefold's queue still decides.
- **Next in the Spotify app** (November 2026): while a Spotify song plays through Soloist, Gatefold's next song, when it's a Spotify song too, goes into Soloist's queue right away (it used to go 10 s before the end). The Spotify app shows it as next, and its Next button skips to it, which Gatefold takes as its own next song. Gatefold's own Next to that song makes Soloist skip to it too, so it isn't left in the queue. Limits: Soloist's queue can't be emptied or reordered, so it holds one song at a time; when Gatefold's next song changes after the hand-over, the old one stays there and ends the current song when Soloist gets to it (Gatefold then plays its own next, with a short gap), and nothing new is handed over until Soloist's queue no longer has it. With a local song next, the app's Next has nothing to skip to; Previous only restarts the song.
- **Lyrics for Spotify songs** (November 2026): from LRCLIB (`/api/get` by title, artist, album, length; once more without the album when that found nothing), shown only. Only for the song that plays and only while the lyrics are on screen; every answer cached (migration 023), misses asked again after 30 days; an hour's pause after a 429 or 5xx (or `Retry-After`), 10 minutes offline; one request at a time. Preferences → Online → Lyrics (on by default, with online lookups). The lyrics pane no longer closes for these songs.

### 9.8 Saved albums, followed artists and discographies
Spotify's albums and artists are shown **apart from the library**, never mixed into its grids, sorting or search:
- **Albums view**: at the end of the album grid, a section "Saved on Spotify (N)", collapsed by default (`show-spotify-albums`), with the saved albums that aren't in the library, as tiles like the library's.
- **Artists view**: at the end of the artist grid, "Followed on Spotify (N)" (`show-spotify-artists`) with followed artists that aren't in the library (by the Spotify ID from the online lookup, or by name). Their page shows Spotify's picture, genres, a Spotify link, and their Albums and EPs & Singles there (no Play All: Spotify's top tracks endpoint is gone).
- **The grids** are list views of rows, each a line of tiles (`SectionedGrid`): GTK's grid view has no sections, and a grid inside a box would build every tile at once. Rows are built only on screen, like the grid view's tiles; they're laid out again when the width changes the number of columns (2 to 12, by the narrowest tile), and a row may get narrower than its tiles so the window can shrink.
- **Library artists' pages**: below their own sections, "On Spotify" lists the releases that aren't in the library. The Spotify ID comes from the online lookup (Wikidata), else a search by name.
- **Album panel**: a Spotify album opens in the same panel. Its songs are library songs where the library has them (sure matches) and songs to stream otherwise (§9.4); play, queue and "Add to Playlist" work as usual; tag and lyrics entries give way to "Open in Spotify".
- **The same release**: same barcode; or an artist in common and the titles agree (§9.5's bracketed names count, edition notes don't); or an artist in common, the same year and song count and a word of the title in common ("樂-STAR" is "ROCK-STAR" on Spotify). Artists' lists have no barcodes, hence the last rule.
- **API** (Development Mode, 2026): `GET /me/albums` and `GET /me/following` still read the library (saving and following moved to `/me/library`); `GET /artists/{id}/albums` with `include_groups=album,single`; `GET /albums/{id}` per album (the batch endpoints are gone). Album labels and popularity, artist followers and popularity were removed from the answers.
- **Requests**: saved albums and followed artists are read with each sync; a discography when its page opens, then kept for a week (migration 013; it was a day until November 2026); album songs for the session. Discographies come 10 releases a request (Development Mode refuses more; Gatefold used to ask for 50 first, a wasted request each time). Spotify still blocked the artists requests for 21 h after about 20 discographies in an evening (November 2026): it seems to count them over a day. Blocks are kept across restarts (`spotify-blocked`); the log counts the requests sent each hour, by kind, and lists them with a block (`journalctl --user -t gatefold`). A library artist's page says under "On Spotify" when Spotify can't be asked (with the wait), instead of leaving the part out. Beside "On Spotify" (on a Spotify artist's page above the releases) the page says when they were read ("Read from Spotify 3 days ago") with a button to ask Spotify again now; if Spotify can't be asked, the stored releases stay and a message says why. With all their releases in the library, the part still shows ("Everything they released on Spotify is in your library"), so new ones can be looked for.
- **Permissions**: `user-library-read` and `user-follow-read`. Logins from before ask to connect again: Spotify answers 403, and the shelves show "Connect Again".
- **Saving and following** (November 2026): a Spotify album's panel has a Save button (+, a check mark once saved) next to Shuffle; artist pages have Follow / Following next to their links, for Spotify artists and for library artists whose Spotify ID is known (the online lookup, or found by name while connected). Both use `PUT`/`DELETE /me/library?uris=spotify:album:…|spotify:artist:…` (the per-type endpoints are gone since February 2026; Spotify's reference lists albums but not artists, its migration guide shows artists too). The change shows at once (the database is updated, the shelves follow); a toast says so with Undo. Permissions `user-library-modify` and `user-follow-modify`; a login without them gets Spotify's 403 and a notice with Connect Again. Preferences → Spotify has Connect Again next to Disconnect (a new login while connected: playlists stay; cancelled, the old login stays in use; another account's login drops the old account's playlists like Disconnect).

---

## 10. Companion Tools: lyrembed, cdport & Ear Tag

lyrembed and cdport (called cdimport before 1.3.0) are separate Python programs installed with pipx, each with a CLI and a local browser page (127.0.0.1 with a secret token). Tags are edited in cdport's tag editor, or in Ear Tag (Flatpak `app.drey.EarTag`) when chosen (§10.3). The player doesn't duplicate what they do. It **starts them**, **opens their pages in the default browser**, and **reads what they write to the files**. A GTK port of the tools isn't planned.

**Contract:** the **command line is the stable interface** (cdport 1.3.0 and newer also has a D-Bus interface, below): arguments, exit codes, `--version`, and the printed line "`<tool> is running at <url>`". Their page JSON APIs (`/api/library`, `/api/jobs`, …) are internal and are **not** called by the player. The player gets results by reading the files afterwards, which it does anyway through inotify. Where the CLI is missing something, §10.4 lists small additions to the tools themselves.

### 10.0 Shared plumbing (`tools/` module)
- **Detection**: look in `PATH`, then `~/.local/bin` explicitly. Apps started from the GNOME menu often don't have `~/.local/bin` in `PATH`, the same issue the lyrembed README notes for fish. Paths can be overridden in Preferences → *Integrations*.
- **Feature probing**: `lyrembed --version` / `cdport --version` → enable features by version, as cdport does with lyrembed. The Integrations page shows *found / version / path* and the install command (`pipx install …`) when a tool is missing. The player never installs them itself.
- **Process control**: `gio::Subprocess` with stdout/stderr pipes read asynchronously on the main loop. To stop a tool, the player sends SIGINT (it behaves as with Ctrl+C, so cdport finishes cyanrip's log), never SIGKILL.
- **Pages in the browser**:
  - Start the tool with `--no-browser` and read the `… is running at http://127.0.0.1:PORT/?token=…` line from stdout. Open it in the default browser with **`gtk::UriLauncher`**.
  - The player keeps the URL while the tool runs. An `AdwBanner` in the library shows *"lyrembed is open in your browser · Show page · Stop"*, or *"Importing a CD with cdport · Show page · Stop"*. **Show page** reopens the address if the tab was lost. **Stop** sends SIGINT.
  - No `--return-url` is passed. The pages keep their normal **Quit** button, and the player reacts to the **process exit**. Closing the tab also ends the tools after a short while (lyrembed), or at the next question (cdport, never during a rip).
  - cdport's hand-over to lyrembed's page and back (its lyrics step) happens entirely in the browser and needs nothing from the player.
- **Audio conflict**: lyrembed's editor plays the track in the page. When a lyrembed page opens, the player **pauses** and remembers the position. It resumes only if the user presses play (no auto-resume over the tool's audio).
- **Picking up results**: tools write into the music files, and inotify triggers a re-read of exactly those tracks. The lyrics pane reloads live without interrupting playback. Writes replace files atomically (new inode), and GStreamer keeps reading the old file descriptor, so this is safe on Linux.
- **cdport over D-Bus** (November 2026): cdport 1.3.0 owns `io.github.sobakah.cdport` on the session bus once `cdport --install-dbus-service` has written its service file (the bus starts it on demand; it exits after a minute without calls or runs). Interface `io.github.sobakah.cdport1` on `/io/github/sobakah/cdport`: `GetInfo` (version, library and working folder, editor: replaces reading its config), `UnfinishedAlbums` (replaces looking for `.cdimport.json` files), `Import({Library, Folder}) → (run, url)` (the page's address to open), `Runs`, `Stop(run)` (as Ctrl+C) and the signal `Finished(run, exit code, album)`. It has no call that runs a command or works on any folder the caller names, so it's fine for a Flatpak (`--talk-name=io.github.sobakah.cdport`).
- **lyrembed over D-Bus** (November 2026): lyrembed 1.2.0 the same way (`lyrembed --install-dbus-service`, `io.github.sobakah.lyrembed1` on `/io/github/sobakah/lyrembed`, idle exit after five minutes): `GetInfo` (version, music folder, romanization layout, max file count, the languages it can romanize; replaces reading its config), `ArtistAliases` (replaces `--list-aliases`; turned into the same text, so the parser and the cache stay), `Romanize(lyrics, han_hint)` (replaces the `romanize --stdio` helper), `OpenPage(folder) → (run, opened)` and `ShowPage(run)` (lyrembed opens the browser itself: the page's address, whose token opens its whole API, never goes over the bus), `FetchLyrics(folder, {Romanize})` (`--auto`, only inside the music folder; elsewhere Gatefold falls back to the program when it sees one), `Runs`, `Stop` and `Finished(run, exit code)`. Gatefold uses it whenever `GetInfo` answers; without it, the program as before.
  - Gatefold uses D-Bus whenever `GetInfo` answers, else starts the program (`cdport`, or `cdimport` for 1.2.x) as before. A run over D-Bus belongs to cdport: closing Gatefold doesn't stop it, and a Gatefold started again shows it in the status bar (`Runs`).
  - **Guidance** (Preferences → Integrations): per tool its status and, when it isn't usable yet, the steps with copyable commands: install pipx (`sudo dnf install pipx`, `pipx ensurepath`), install the tool from its repository with pipx, `cdport --install-dbus-service` (a *Set Up* button runs it where Gatefold sees the program), switching from cdimport (`pipx uninstall cdimport` first: both install a `cdimport` command), repairing a service whose program is gone. A tool that's needed but missing shows a toast with *How to Set Up*. *Look Again* (and opening Preferences) detects anew. Both tools have the same steps (switching from an older lyrembed: `pipx upgrade lyrembed`).
- **Native app**: the RPM build (§12) starts the tools directly. No `flatpak-spawn --host` or sandbox permissions are needed. Ear Tag, as a Flatpak, is started through `flatpak run` (§10.3).

### 10.1 lyrembed: lyrics search, checking and romanization

| Where in the player | What happens | Command |
|---|---|---|
| Lyrics pane, empty state *"No lyrics"* | **Find automatically**: background job, a spinner in the pane, then a toast with the result | `lyrembed <album dir> --auto [--romanize]` |
| Lyrics pane / track row ⋮ | **Check & edit in lyrembed…**: lyrembed's page for the album in the browser | `lyrembed <album dir> --no-browser` |
| Album panel ⋮ | **Fetch missing lyrics for album**; **Save romanization to files** (§5.1) | `lyrembed <album dir> --auto`; romanizing: the page's bulk action until a CLI option exists (§10.4) |
| In the background | **Automatic romanization** for display (§5.1) | `lyrembed romanize --stdio` (new, §10.4) |
| Library ⋮ (later) | **Fetch missing lyrics for the whole library**: a queue of `--auto` jobs, **one album folder per call** (lyrembed refuses folders over `max_file_count`, 250 by default, and goes at most 3 levels deep), one job at a time, stoppable | per album |

- **Exit codes** are interpreted as follows: `0` done (which can also mean "nothing confident found", so the player re-reads the tracks to see which still have no lyrics, as cdport does), `1` some files failed, `2` unusable path or config, `130` cancelled. Tracks still without lyrics after `--auto` get a *"Choose manually in lyrembed…"* action.
- **The romanization layout matters for dual-line display**: lyrembed's default `romanization_layout` is `"replace"`, which drops the original script. Preferences → Integrations shows a hint if lyrembed's config (`~/.config/lyrembed/config.json`) doesn't say `"below"`. The player only reads that file and never writes it.
- **Artist aliases**: lyrembed has about 205 alias groups (K-pop, J-pop, C-pop, spellings with and without diacritics) plus the user's own. The player reads them through `lyrembed --list-aliases` (§10.4 proposes JSON output) and caches them. It uses them for:
  - search ("Hoshino Gen" finds 星野源),
  - artist merging (§3.3 rule 4),
  - showing the Latin name as a subtitle under a native-script artist name on the artist page.
- **Romanization**: automatic, with lyrembed's engine. See §5.1.

### 10.2 cdport: getting CDs into the library

| Where in the player | What happens | Command |
|---|---|---|
| Library main menu → **Import CD…** | Opens cdport's page in the browser, plus a status banner in the player | `cdport --web --no-browser -l <library folder>` |
| Audio CD inserted (GIO `GVolumeMonitor`, content type `x-content/audio-cdda`) | `AdwBanner`: *"Audio CD detected — Import"* | as above |
| Unfinished imports | Banner: *"1 unfinished CD import — Continue"*. The player finds `.cdport.json` files in cdport's `work_dir` (read from `~/.config/cdport/config.json`) and only checks whether they exist, never their contents | `cdport <album dir> --web --no-browser …` |
| After the import | cdport moves the album to `<library>/<albumartist>/<folder>`. inotify sees the new folder, and the player shows a toast *"New album: \<title\> — Play / Show"* | — |

- **Library folder agreement**: on first run, if cdport is installed, the player reads its `library_dir` and offers to add it as a library folder. When starting cdport, the player passes `-l` with its own primary folder, so imports always land where the player looks.
- **Multi-disc merges**: cdport can add disc 2 into disc 1's existing folder. The scanner treats new files in a known album folder as part of that album (same release MBID / album + album artist), not as a new album.
- **Debounce**: an album appears as a folder move (atomic on one filesystem), but cover and cue files can follow shortly after. The scanner waits ~2 s after the last event in a folder before reading it.
- **Covers**: cdport writes `cover.jpg` and an embedded front cover, which the cover pipeline (§4.3) uses without extra work.
- cdport already chains lyrembed for the lyrics step, so the player adds nothing there.

### 10.3 Tag editing: cdport's editor, or Ear Tag
The player never writes tags itself (Q8). Every editing action opens the album in a tag editor: by default **cdport's own** (cdport 1.4.0 on), a page in the browser like its imports; in the RPM build **Ear Tag** (Flatpak `app.drey.EarTag`), Kid3, MusicBrainz Picard or a custom command can be chosen instead (Preferences → Integrations → *Tag Editor*). The Flatpak always uses cdport's: it can't start the others, and reaches cdport over D-Bus (decided 2026-11-10; before, Ear Tag was the default and the Flatpak couldn't edit tags at all).

| Where in the player | Action | Files passed |
|---|---|---|
| Album panel ⋮ | **Edit Tags…** | all tracks of the album (of the chosen edition) |
| Track row ⋮ (expanded song details) | **Edit tags…** | that track |
| Artist page | **Edit Album Artist Tags…** (e.g. to set `ALBUMARTISTS` for a real split, §3.3) | all tracks of that artist's albums |
| Lyrics pane ⋮ | — (lyrics stay with lyrembed, §10.1) | — |

**cdport's editor**
- **Over D-Bus**: `EditTags(s folder, a{sv} options) → (s run, s url)`, a page run like `Import` (§10.2): Gatefold opens the address, the status bar says *Editing tags with cdport* with *Show Page* and *Stop*, and it's one page at a time with lyrembed's and imports. Without the service: `cdport FOLDER --edit-tags --web --no-browser`, started by Gatefold.
- **One album folder**: the files' folder (the first, when they're in several: an artist's albums, disc folders; a notice says so). Which formats it edits is cdport's to say (1.4.0: FLAC, MP3, Ogg Vorbis, Opus and M4A, also mixed in one album), as is where: its service only accepts folders inside its library, its working folder or the music folder. A refusal shows with cdport's reason ("cdport couldn't start: …"); without the service, the program says so on its page. So a cdport that takes more formats needs no change here.
- **Not an import**: no "New album" notices follow (a run that survived a Gatefold restart can't be told apart and still counts as one).
- Older cdport: a notice asks for 1.4.0 (`pipx upgrade cdport`).

**Ear Tag and the others (RPM)**
- **Command**: `flatpak run --file-forwarding app.drey.EarTag @@ <files> @@`, the same template as cdport's `editors` config (`{files}`, `{dir}`). File forwarding hands the files over through the document portal, so Ear Tag can write them even outside its sandbox permissions.
- **Detection**: `flatpak info app.drey.EarTag` (user or system installation). If it's missing, *Edit Tags* opens GNOME Software (`appstream://app.drey.EarTag`).
- **No waiting**: Ear Tag returns at once, especially when its window is already open, so the player doesn't wait for the process.

**Both**: saved changes arrive through inotify, and the affected tracks/albums are re-read and re-grouped (album artist, editions, release type); an album can move to another artist live. During playback the current track keeps playing, and the new metadata shows when the re-read finishes (as with lyrembed's writes, §10.0).

### 10.4 Proposed additions to the tools

These are small changes in lyrembed and cdimport that would make the integration cleaner. **The first one is required for automatic romanization (§5.1).** The others are optional: the player works without them, with fewer features.

| Tool | Addition | Lets the player… |
|---|---|---|
| lyrembed | **Done in 1.1.1:** `lyrembed romanize --stdio`, a long-running filter. Input: JSON lines `{"id":…, "lyrics":"…", "han_hint":"ja"\|"zh"\|null}`. Output: `{"id":…, "lyrics":"…" (layout below) \| null, "language":"ko"\|"ja"\|"zh"\|"mixed"\|"any"\|null, "reason":"…"}`, or `"error"` for a line that isn't a request. Writes nothing, no network. | romanize automatically for display (§5.1) with lyrembed's engine, with ~1 s start-up once instead of per song |
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
- **Other desktops** (KDE Plasma, Xfce, Cinnamon, window managers; checked 2026-11-09). Other distributions are reached through the Flatpak (phase 9), not by lowering the GTK/libadwaita requirement.
  - *Icons*: every symbolic icon Gatefold uses is bundled (`data/resources/icons`, copied from adwaita-icon-theme). The desktop's icon theme still wins where it has the icon; the bundle fills its gaps (KDE's Breeze lacks six of them, e.g. the queue button and the drag handles). A test (`icons.rs`) fails when an icon is used but not bundled.
  - *Light or dark*: Preferences › Library › Appearance › Style: Follow System, Light or Dark. On desktops other than GNOME and KDE (which report light or dark through the settings portal, and whose legacy GTK theme mustn't win), a dark GTK theme gives the dark style with Follow System. GTK's `gtk-theme-name` can't tell (libadwaita sets it to its empty theme), so the name comes from `GTK_THEME`, the desktop's GSettings (`org.cinnamon.desktop.interface`, `org.mate.interface`, `org.gnome.desktop.interface`: `gtk-theme`), Xfce's `xsettings.xml` (`/Net/ThemeName`) or GTK's `settings.ini` (also `gtk-application-prefer-dark-theme`).
  - *Keyring*: without a Secret Service (GNOME Keyring, KWallet, KeePassXC…) or, in a Flatpak, the secret portal, the Spotify logins and the Soloist key go to `logins.json` in Gatefold's data folder (0600, written atomically) instead of being lost at every restart; Preferences › Spotify says so. A keyring that appears later takes them over at the next save.

---

## 12. Build & Packaging (RPM, local)
For now the app is built as an **RPM for Fedora 45**, attached to GitHub releases. A Flatpak and real distribution are phase 9 (§13). COPR stays an easy step later, because the same spec file works there.

- **Build system**: Meson + Cargo (GNOME Rust app layout). The spec uses the standard `%meson` / `%meson_build` / `%meson_install` macros.
- **Rust dependencies are vendored**: `cargo vendor` produces a second source tarball (`Source1`); `%cargo_prep -v vendor` points Cargo at it and turns off network access, so the build runs offline. Many crates used here (lofty, bliss-audio, mpris-server, oo7, librespot) aren't packaged in Fedora, so vendoring is the practical way. `Cargo.lock` is committed.
- **Clean builds** with `mock -r fedora-45-x86_64` (catches missing BuildRequires), quick local builds with `rpmbuild -ba`. A `make rpm`/`just rpm` target wraps both.
- **Build conditions**: `%bcond spotify_stream 0` → `rpmbuild --with spotify_stream` enables the librespot Cargo feature (§9.4). The default build contains no librespot code.

| | Packages (Fedora 45) |
|---|---|
| **BuildRequires** | `meson`, `cargo`, `rust`, `blueprint-compiler`, `gtk4-devel`, `libadwaita-devel`, `gstreamer1-devel`, `gstreamer1-plugins-base-devel`, `sqlite-devel` (or bundled SQLite via rusqlite), `desktop-file-utils`, `appstream` (`appstreamcli`), `gettext`. Plus the FFmpeg dev libraries from `ffmpeg-free-devel` if bliss uses its FFmpeg decoder, see below. |
| **Requires** | `gtk4`, `libadwaita`, `glycin-loaders`, `gstreamer1-plugins-base` (`playbin3`), `gstreamer1-plugins-good` (FLAC, `rgvolume` ReplayGain), `gstreamer1-plugins-bad-free` (`bpmdetect`/SoundTouch), `gstreamer1-plugin-libav` (AAC/ALAC in the M4A files) |
| **Recommends** | `gstreamer1-plugins-ugly-free` (WMA via `asfdemux`, not needed for `~/Musik` today), `flatpak` (for Ear Tag) |
| **Not RPM dependencies** | lyrembed, cdport (pipx), Ear Tag (Flatpak): detected at runtime (§10) |

- **Installed files**: `/usr/bin/gatefold`, the desktop file and AppStream metainfo (validated in `%check`), the GSettings schema, icons (scalable + symbolic), the GResource bundle, and translations.
- **bliss decoding** (verify in an early spike): bliss-audio decodes through FFmpeg by default. On Fedora that means `ffmpeg-free`, which has to decode the library's FLAC, AAC/ALAC (M4A) and MP3 files. The alternative is feeding bliss samples decoded by GStreamer, so there's one decoding stack, and `bpmdetect` can run in the same pipeline pass. That's preferred if the bliss version supports external samples.
- **Name and IDs**: the app is called **Gatefold**.

  | What | Value |
  |---|---|
  | App ID (desktop file, metainfo, GSettings schema, D-Bus name, icons) | `io.github.sobakah.Gatefold` |
  | MPRIS bus name | `org.mpris.MediaPlayer2.Gatefold` (`…Gatefold.Devel` for the development build) |
  | Command / binary / RPM package | `gatefold` (spec file `gatefold.spec`) |
  | Data (DB, portraits, analysis) | `~/.local/share/io.github.sobakah.Gatefold/` |
  | Cache (cover thumbnails) | `~/.cache/io.github.sobakah.Gatefold/` |
  | Keyring entries (Spotify tokens) | attribute `application = <app ID>` (the development build has its own; it copied the shared entries once when it got them) |
  | Rust crates | `gatefold` (app), `gatefold-core` (library, playback, analysis and similarity), `gatefold-metadata`, `gatefold-lyrics`, `gatefold-tools`, `gatefold-spotify`, `gatefold-soloist`, `gatefold-stream` (librespot, optional) |
  | User-Agent for MusicBrainz/Wikidata/Deezer | `Gatefold/<version> ( https://github.com/sobakah/gatefold )` |

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
| **9: Flatpak & distribution** | Flatpak manifest and CI build, host programs through `flatpak-spawn --host`, data migration from the RPM install, a Flatpak repository with updates through GNOME Software (the update notice for the RPM is already built, §13, Phase 9 plan) |

### Phase 9 plan: Flatpak & distribution (planned 2026-11-08)
- **Runtime**: GNOME 51 has GTK 4.24, GStreamer 1.28, SQLite, libadwaita, the playback plugins (incl. replaygain, libav, pipewire, spectrum) and glycin. Missing is soundtouch (bpmdetect for the analysis: soundtouch and gst-plugins-bad's plugin as modules); AAC may need the codecs extension. Rust builds offline from a generated `cargo-sources.json`. oo7 uses the Secret portal by itself (one new Spotify login), browser and file dialogs use portals, the network works as is.
- **Host programs** keep working without changes to them or to how they're installed (pipx, Ear Tag's Flatpak, soloistd's RPM): lyrembed incl. the romanizer, cdimport, Ear Tag's `flatpak run`, Soloist, soloistd's `systemctl`/`journalctl`/`soloistd update` and its config.json all go through `flatpak-spawn --host` (`--talk-name=org.freedesktop.Flatpak`), found on the host, not in the sandbox's PATH. Soloist is stopped with Gatefold through `--watch-bus` instead of PR_SET_PDEATHSIG. Tool pages open in the browser as before.
- **Files**: `--filesystem=xdg-music:ro`; other library folders by permission (portal paths break file watching and the tools, so real paths: `--filesystem=home:ro` is the simple alternative); gvfs for drive and CD detection. Writes into the music files only happen through the tools on the host.
- **Data**: in `~/.var/app/io.github.sobakah.Gatefold/`; on first start the RPM install's library database, portraits and settings are copied over once (the Spotify login is asked again, the keyring entries belong to the new app).
- **D-Bus**: `--own-name=org.mpris.MediaPlayer2.Gatefold`, `--talk-name=org.freedesktop.UPower`.
- **Test Flatpak** (2026-11-09): `make flatpak` builds the working tree as `io.github.sobakah.Gatefold.Devel` (GNOME 51 runtime, SoundTouch and gst-plugins-bad's soundtouch plugin for `bpmdetect`, the music folder read-only) and installs it for the user; `make flatpak-bundle` writes a `.flatpak` file. Cargo fetches crates during the build, so it isn't Flathub-ready; the host programs above don't work in it yet. See build-aux/flatpak/README.md.
- **Distribution**: CI builds the Flatpak next to the RPM. A `.flatpak` bundle on the GitHub release installs, but doesn't update. For updates: a Flatpak repository (ostree, signed with a GPG key, hosted on GitHub Pages or similar) plus a `.flatpakref`; GNOME Software then updates Gatefold like any other app. GitHub Pages needs a public repository (or a paid plan); until then the bundle, installed again by hand.
- **Update notice** (built early, November 2026; `updates.rs`): when Gatefold starts (after 8 s) and every 12 hours, with Preferences → Online → *Check for Updates* on (`check-updates`). A banner names the first update found; *Details…* shows the notes with *Install*, *Release Page*, *Skip This Version* (`skipped-updates`) and *Later* (this session). Nothing is installed without that choice.
  - **Gatefold** (only the RPM build in `/usr/bin`, not development builds, not with `-Dupdate_check=false`, which a Flatpak build sets): the newest release above this version, pre-releases included. Asked through `gh api` while the repository is private (gh's own login; Gatefold never sees the token), else GitHub's API directly (enough once it's public). *Install* downloads this Fedora release's RPM (through `gh release download`, or HTTPS), checks it against the SHA-256 GitHub lists for the asset (refused without one or when it differs), installs it with `pkexec dnf install -y` (GNOME's password dialog; cancelling does nothing) and offers *Restart Now* (Gatefold quits normally and a small shell starts it again once this process is gone).
  - **Soloist** (soloistd's): Spotify's archive on `soloist-builds.spotifycdn.com` has another ETag than soloistd's `release.json` (a HEAD request). *Update* runs `soloistd update` and restarts the service; Gatefold reconnects by itself. soloistd also updates on its own every six hours, which Gatefold can't stop (soloistd has no setting for it).
  - **soloistd**: a newer release on GitHub (`MopigamesYT/soloistd`) than `soloistd version`; installed like Gatefold's RPM, with its digest.
  - Testing: `GATEFOLD_UPDATE_TEST=0.1.0` pretends to be that version (and checks in development builds), `GATEFOLD_UPDATE_REPO=owner/name` asks a public repository instead, `GATEFOLD_SCREENSHOT_UPDATE=1` opens the dialog after the check.
- **Soloist in the Flatpak** (decided 2026-11-08): Gatefold manages Soloist itself; soloistd isn't bundled.
  - *Why not bundle soloistd:* it would be allowed (MPL-2.0: ship it unchanged with its license and a link to its source), and it wouldn't make Gatefold distribute Soloist, since soloistd downloads Soloist on the user's machine from Spotify's server with the user's own API key. But in the sandbox it can't be a systemd user service (Gatefold would run `soloistd run` while it runs), and it updates Soloist every six hours without asking. Gatefold already starts Soloist itself; only the download is missing.
  - *Gatefold downloads Soloist itself*, only when the user agrees (first setup and each new build, like the update notice): the archive for the machine (`soloist_release_x86_64.tar.gz`, `…_arm64`, `…_arm32`) from `soloist-builds.spotifycdn.com`, checked by ETag (HEAD) against the one installed, unpacked into the app's data folder, validated with `soloist --version` and swapped in atomically; Soloist is restarted gracefully. A build that expired (exit code 10) asks right away. Still no Soloist code in Gatefold's package: the user's machine fetches it from Spotify.
  - *Differences to soloistd on the host:* the Spotify device exists only while Gatefold runs (Background portal to keep it while the window is closed); the sandboxed Soloist has its own data folder, so it's linked once in the Spotify app; it needs `--socket=pulseaudio` (or PipeWire) and the network.
  - *Test Flatpak, for now (2026-11-10):* until the download exists, it runs the Soloist soloistd keeps on the host (`--filesystem=xdg-data/soloistd/bin:ro`) as its own Soloist, with its own API key and the device name "Gatefold Devel"; it doesn't attach to the host's soloistd (no access to its config.json), because soloistd manages only one Soloist and the user keeps that for the installed app. Preferences hides the soloistd group in a Flatpak.
  - *A soloistd on the host keeps working:* Gatefold attaches to its Soloist over 127.0.0.1 (the sandbox shares the network) and reads its config.json (`--filesystem=xdg-config/soloistd:ro`); controlling the service (`systemctl`, `soloistd update`) goes through `flatpak-spawn --host`, as now.
- **The companion tools as Flatpaks**: from our own Flatpak they run either way (`flatpak-spawn --host` starts pipx programs and `flatpak run` alike), so nothing has to change in them. For Flathub, being a Flatpak isn't enough: a Flathub app can neither start host programs nor run other Flatpaks directly. Ear Tag works through the "Open With" portal (files handed over). lyrembed and cdport each needed a D-Bus interface Gatefold may call (`--talk-name=<their name>`); cdport has it since 1.3.0, lyrembed since 1.2.0 (§10.0). They can stay Python programs installed with pipx on the host: each owns a name on the session bus (e.g. through `jeepney` or `dbus-fast`, both pure Python from PyPI) and installs a D-Bus service file in `~/.local/share/dbus-1/services/`, so the bus starts it when Gatefold calls (the sandbox can't start host programs, but the bus can). Paths are the same on both sides (Gatefold sees `xdg-music`), the romanizer's `--stdio` exchange becomes a method call, and a tool's page is opened by asking the tool for its URL (with the token) and handing that to the browser. Turning them into Flatpaks isn't needed; it would add work (their Flatpaks would need the music folder, cdimport the CD drive, `--device=all`, and cyanrip inside it). Such an interface mustn't be a way out of the sandbox (no "run this command" method), or Flathub's review would treat it like `flatpak-spawn --host`.
- **App icon** (2026-11-09): an LP in a gatefold sleeve, its front cover slightly opened (in perspective) and the record halfway out from between the panels; the cover nods to *The Dark Side of the Moon* (prism, light beam, spectrum). Before publishing widely (Flathub, a public repository), check that the nod is far enough from the original artwork and Pink Floyd's prism trademark, or switch to an abstract cover of our own.
- **Flathub** is a separate decision: it rejects `flatpak-spawn --host` and needs a public repository. As far as known, it also doesn't accept apps that download and run program code after installation; its route for proprietary programs (extra-data) needs a fixed checksum, and Soloist's download changes with every build. A Flathub version would go without the companion tools (unless they get D-Bus interfaces, above) and soloistd's controls, and without downloading Soloist: Ear Tag through "Open With", Spotify through an already running Soloist over its WebSocket only (Soloist can't be shipped: Spotify forbids redistributing it).
- **RPM** stays: COPR (same spec, needs a public repository in practice) would give dnf/GNOME Software updates for it too.
- **Effort**: two to three sessions (manifest and CI; host programs; migration, permissions, a full test), plus one for the repository and one for Gatefold's own Soloist download.

The tool additions in §10.4 can be done in lyrembed/cdimport at any time. Phase 4 works without them, except automatic romanization.

### Phase 8 as built (so far)
- Crate `gatefold-stream` (only with the `spotify-stream` feature): librespot 0.8 on its own Tokio thread; its decoded samples go through Gatefold's own GStreamer pipeline (`appsrc ! audioconvert ! audioresample ! volume ! autoaudiosink`), with the same volume as local playback.
- Login: a second, one-time browser login with librespot's client ID (the user's Web API app can't stream), redirect `http://127.0.0.1:5588/login`, scope `streaming`. librespot's reusable credentials then live in the keyring; later starts need no browser.
- Spotify songs that aren't in the library get negative track IDs (table `stream_track`), so they go into the queue, playlists and the saved queue like library songs. The player streams them and plays everything else from the files; the next streamed song is preloaded.
- Loudness: with ReplayGain on, librespot normalises with a −4 dB pregain (Spotify's −14 LUFS to ReplayGain's −18 LUFS).
- Development builds (`make run`) include the feature; it's off until turned on in Preferences → Spotify → Streaming.
- **librespot fails on some accounts** (Spotify refuses its audio keys, librespot issue #1649, open since Nov 2025). It stays as an option.
- **Spotify Soloist** (Spotify's official headless Connect client, Aug 2026) is the default way to stream now: crate `gatefold-soloist` drives Soloist's local WebSocket API (play a URI, pause, seek, volume, shuffle/repeat; events for track, status, position). Soloist plays the audio itself (PipeWire/PulseAudio), so Gatefold's ReplayGain and pipeline don't apply to it; Gatefold sets its volume. Gatefold uses a Soloist that already runs (soloistd's, found through `websocket` in its config.json, or Soloist's runtime files), or starts one itself with the user's API key from the keyring (own data folder, device name "Gatefold", WebSocket on 127.0.0.1 only). Soloist can't be shipped with Gatefold (Spotify forbids redistributing it); builds expire after 90 days (exit code 10, which Gatefold reports; soloistd updates by itself). Linking happens once in the Spotify app's device list. Gatefold's queue decides what follows: when Soloist moves on by itself (autoplay), Gatefold pauses it and plays its own next song. Spotify's terms don't clearly cover controlling Soloist from another app (Music Assistant says the same).
- **soloistd in Preferences**: the systemd user service (state, start/stop/restart), its config.json (WebSocket for Gatefold, device name, PipeWire output, volume at start, API key; written 0600, unknown keys kept, the service restarts), `soloistd update`, linking again (`soloistd pair` with the service stopped) and the journal.
- Soloist is linked once (its session stays in its data folder); after a restart Gatefold activates it before playing. A Soloist Gatefold started stops with Gatefold (also if it dies: PR_SET_PDEATHSIG).
- **Receiver**: when Soloist plays something Gatefold didn't start (cast from the Spotify app, or a followed playlist), Gatefold shows it like remote mode and its controls steer Soloist; a short quiet time after Gatefold pauses Soloist keeps Soloist's own next song from counting.
- **Followed playlists** come last in the Playlists view, hidden unless asked for; with Soloist, Play starts them there (their songs still can't be listed: Web API).
- **Spotify in search** (November 2026): Spotify's songs, artists, albums and playlists come into the search card's sections by kind (one request, `type=track,artist,album,playlist`, 10 each), after the library's own results; Spotify's matches of library items (songs surely matched, artists by ID or name, albums as the same release, linked playlists) show as the library's. An icon at the end of every row says in the library or only on Spotify. Spotify songs: play, queue, add to playlists (local playlists hold them as Spotify entries, linked playlists get them on Spotify too); artists and albums open their Spotify page or panel in Gatefold; others' playlists play with Soloist or open in Spotify (their songs aren't shown to apps). Spotify is asked 450 ms after typing stops, each query once a session. The last five searches (opened a result, or closed the search with it) show when the field is empty (`recent-searches`).
- **Transitions**: Gatefold's next Spotify song goes into Soloist's queue (since November 2026 as soon as the current one plays, §9.7), so it follows seamlessly; with a local song next, Soloist is muted for its last 0.4 s. **Loudness**: Spotify songs in Soloist are lowered by an adjustable offset (default −4 dB, Spotify's −14 LUFS to ReplayGain's −18).
- **Covers**: Spotify images at their largest (640 px).
- Not planned: gapless between local and streamed songs (two players), and reading followed playlists' songs (only librespot could).

### Playback additions (after phase 8)
- **Loudness**: a fixed ReplayGain pre-amp of +4 dB (ReplayGain's −18 LUFS reference up to Spotify's and most players' −14 LUFS; a setting until 0.2.1, removed once local and Spotify songs sounded equally loud); songs without tags get the gain their loudness calls for, measured by the song analysis (BS.1770 on its mono signal, +3.6 dB for stereo, within about 0.5 dB of the tags of the same files; an album in order gets one gain for all its songs, so gapless changes stay seamless); unmeasured ones stay as they are. `rglimiter` prevents clipping. Spotify songs in Soloist then need no offset (default 0 dB).
- **Smart crossfade** (on by default when crossfade is on): the analysis also measures each song's edges from its loudness in 50 ms windows — where leading silence ends (−40 dB below its 90th-percentile loudness), where it gets quiet (one-second average 15 dB below: a fade-out or quiet ending starts) and where it's over (−40 dB). The fade runs from "quiet" to "over" (at least 0.8 s, at most the crossfade setting), and the next song starts after its silence (at most 10 s skipped). Without edges (analysis off, not analyzed yet) the last seconds fade. Analysis features version 3 (all songs analyzed once more).
- **Crossfade** (0–12 s, off by default): two GStreamer engines; near the end of a song the next one starts on the other engine and they fade with an equal-power curve; the player follows the new engine, events of the fading one are ignored, any transport action ends the fade. Not for Spotify songs (Soloist plays itself), Repeat One or very short songs, and, unless chosen, not within an album playing in order (stays gapless).

### Optional future features (not scheduled)
Ideas that came up along the way, to pick from once the roadmap is done:

### Known issues (code reviews, November 2026)
A review of the whole program (six areas, read in full) found about 85 problems; all but the Soloist key below are fixed (the last ones on 2026-11-08: scans now write only what changed, and the reload after a scan runs on a worker thread).

A second one on 2026-11-10 (six rounds by risk, every file read in full: database and scanner, playback and queue, Spotify and streaming, companion tools and lyrics, the app and its widgets, metadata and matching) found 87 more; 85 are fixed (*Edit Tags* in the Flatpak by cdport's tag editor, §10.3), these two are left:

**Slow with large libraries (left for later)**
- **Search reads every match** for one- and two-letter queries (an unindexed `LIKE` over the whole index) and only then keeps the first few of each kind, on every keystroke. Where: `core/src/db.rs`, `search`.
- **Tags are read twice per file** while scanning: once by lofty in general, once more by the format's reader just for `RELEASECOMMENT`. Doubles the tag reading of a first scan (felt most on network and USB drives). Where: `core/src/library/tags.rs`, `read_release_comment`.

**Security**
- **Soloist's API key is on its command line** (`soloist --api-key …`), which other local users can read (`ps`, `/proc/<pid>/cmdline`). Soloist (build of 2026-11-06) has no other way: no environment variable, key file or stdin; spotify/soloist issues #12 and #24 ask for one. soloistd passes it the same way. Small risk on a single-user machine. *Ready:* when `soloist --help` lists `--api-key-file`, Gatefold writes the key to a 0600 file next to its Soloist data folder, passes that, and removes the file once Soloist listens (chosen over an environment variable, which child processes inherit and crash reports may contain). soloistd would need the same change (or systemd's `LoadCredential=`). Where: `soloist/src/lib.rs`, `Soloist::start`.

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
- **Q5** Grids: library tiles show cover, album, artist. Artist page tiles show cover, album. Cover size set in Preferences, kept at every width (§4.1, §4.2; first the size adapted to the width, which made tiles grow and shrink noticeably while resizing).
- **Q7** Scrobbling: no.
- **Q8** No app-only metadata overrides (except portraits and Spotify match choices). Tag and cover editing goes to Ear Tag (§10.3).
- **Q9** Distribution: RPM on GitHub releases for now (§12); a Flatpak with updates is phase 9 (§13).
- **Q15** Name: **Gatefold**, app ID `io.github.sobakah.Gatefold` (§12).

### Still open
None at the moment.
