# Changelog

All notable changes to Jewelcase are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and versions follow
[Semantic Versioning](https://semver.org/).

## [Unreleased]

### Added

- Songs in the queue and under Recently Played open their album or artist from their ⋮ menu (*Go to Album*, *Go to Artist*).

### Changed

- **Every song in the queue has the same ⋮ menu**: Now Playing and the songs coming up have one too, like Recently Played. The right-click menus are gone (on Recently Played they repeated the ⋮ menu).
- *Add to Playlist…* opens as a menu next to where you chose it and closes when you click beside it, like Jewelcase's other menus.
- The *Play On…* button shows a speaker instead of an icon that looked like a printer.

## [0.10.0] — 2026-11-21

The library database changes (artist pages remember when only their first releases from Spotify
are stored); 0.9.0 can't open it any more.

### Changed

- **The album beside an artist page stays longer**: as the window gets narrower, the artist grid first gives up its room, down to two columns of artists; then the artist page and the album on it get narrower together, at the same rate; then the page, with the album still beside it, goes over the artist grid. Only when the window is too narrow for the page and the album side by side does the album cover the page, using the whole window; it moves back beside the page when the window is wide enough again. Before, the page got narrower while the album kept its width, and the album went over the page early.
- The album and playlist panels likewise leave the album grid two columns before they get narrower.
- **Playlists glide to their new places** too, like albums and artists, when a playlist opens beside them or the window gets wider or narrower; followed playlists are a section that opens and closes like *Followed on Spotify*.
- A panel beside a grid or an artist page is at least 360 px wide, so song titles aren't cut to a few letters; with less room it covers what's beside it.
- **What you opened last stays in view on narrow windows**: an open album or playlist no longer closes when the window gets narrow; when there's no room for it beside the grid, it covers the grid, using the whole window. Opened on a narrow window, it also takes the whole window, instead of a strip too narrow for its song titles.
- **Artist pages ask Spotify much less**: a page shows the first 10 of the artist's releases on Spotify, with *Show All* below them for the rest. Spotify allows only so many requests about artists a day, shared by every app on your account, and reading all of a prolific artist's releases took up to nine; a few dozen artist pages in an evening could block them for the rest of the day.

### Fixed

- Narrowing or widening the window with an artist page and an album open no longer shows the album over part of the page, nor the page through the album.
- An artist page no longer covers part of the artist grid on windows just wide enough for both.
- **Nothing is cut off at the window's edge any more**: on some window widths, the window cut off its right side (the header bar's buttons, the player bar, an open album). Now the tabs move to the bottom when the header bar has no room for them, the player bar turns compact when the full one doesn't fit, and the grids go down to one column. With the lyrics or the queue open in the full player, the player takes its narrow layout (the lyrics or queue in place of the cover) when they don't fit beside the controls; before, everything on the right was cut off on windows between about 640 and 900 px wide, also with the player closed. In the full player on a very narrow window, the Lyrics and Queue buttons move up to its header bar.

## [0.9.0] — 2026-11-20

The library database doesn't change; 0.8.0 still opens it.

### Changed

- **Large playlists stay smooth**: scrolling through a playlist of thousands of songs no longer stutters, it opens at once instead of filling in over a few seconds, and adding, removing, moving or searching songs in it no longer makes the window hang.
- Changing one playlist, or syncing with Spotify, no longer rebuilds the tiles of all the others.
- **Albums glide to their new places** when an album opens or closes beside the grid, or the window gets wider or narrower, instead of jumping there; the albums at the top stay in view. While you drag a panel's edge to resize it, the albums or artists hold still and glide to their places once you let go.
- The songs of a Spotify album fade in once they have arrived instead of appearing all at once.
- **Artist pages open beside the artist grid**, like an album beside the album grid, so you can open another artist without closing the page first. A page is at least 560 px wide; on windows narrower than 960 px it opens over the grid as before. On a narrow artist page an album opens over the page instead of squeezing it.

### Fixed

- The Spotify icon before a song with an artist line under it (another artist, a guest) is in the middle of the row again, beside both lines, instead of higher than the number and the buttons (album songs, the queue).
- A Spotify album shows its cover also when nothing showed it before, such as a new release opened from the bell; it stayed a placeholder until the album had been seen on its artist's page.
- Lyrics of a Spotify song start at their first line; they could open in the middle or near the end and jump to the top when the singing began.

## [0.8.0] — 2026-11-14

The library database doesn't change; 0.7.0 still opens it.

### Added

- **Choose the app icon's disc**: Preferences → Library → *App Icon* offers nine discs for the icon's jewel case, shown in the dash and the app grid within a few seconds.
- **A Flatpak for Fedora Silverblue, Bluefin and other image-based systems**: each release has a `.flatpak` file next to the RPM. Its update notice points to the next release's file.
- On image-based systems, Preferences → Integrations shows how to install lyrembed there (pipx into the home folder) and cdport (in a toolbox).

### Fixed

- Next on a Spotify song no longer skips the song after it as well.
- The mini player's visualizer sits in the middle of the space between the song and the controls again, not to the right of it.

## [0.7.0] — 2026-11-13

The library database changes (new releases can have several artists; playlists remember the
library-only filter); 0.6.1 can't open it any more.

### Added

- **Only the songs you own in a playlist**: *Only Songs in My Library* in a playlist's sort menu hides the songs that would come from Spotify; Play, Shuffle and the queue take only the songs shown. Each playlist remembers it.

### Changed

- The player has no × any more; close it with the grip on top, by dragging it down, with Esc or by clicking beside it.

### Fixed

- Spotify songs no longer jump to the next song when you seek in them after Next or after picking a song in the queue.
- Clicking beside the player closes it again after songs were dragged in the queue; before, only the grip did until a restart.
- The player shows no × also when your button layout puts it on the left.
- New releases: an album named after a single released before it is shown; it was taken for a reissue.
- New releases: a release by two artists you follow is listed once with both names, and stays when you unfollow one of them.
- At the end of a Spotify song that Soloist starts once more, Jewelcase now always goes on to the next song.
- Searching a long playlist is quicker.

## [0.6.1] — 2026-11-12

The library database is unchanged; 0.6.0 still opens it.

### Fixed

- The app icon is as large as other apps' icons in the dock and the app grid, instead of a size smaller.

- The Spotify icon before songs from Spotify is back to the size of the other icons, and the artist under such a song now starts under the title instead of under the icon (mini player, queue, album songs).

## [0.6.0] — 2026-11-12

The library database gets two new tables (for new releases); 0.5.1 can't open it any more.

### Added

- **Sort artists** by name or by date added, from the sort button on the Artists page.

- **New releases of the artists you follow on Spotify**: a bell in the header bar lists their albums, EPs and singles from the last 30 days, read from Deezer once a day; a dot on it means something new. Clicking one opens it from Spotify. Preferences → Online → *New Releases of Followed Artists* (needs *Look Up Artists Online*).

### Changed

- Albums saved on Spotify and artists followed there are sorted like your own albums and artists, instead of in the order you saved or followed them.

- **A new app icon**: a clear CD case with the disc inside, drawn flat like GNOME's own icons.

- Songs, albums and playlists from Spotify are marked with Spotify's official icon, in black or white and a little larger, as Spotify's guidelines ask.

### Fixed

- The cover cache took about twice the space it needs: covers and artist pictures smaller than a thumbnail size were stored again at each bigger size (every Spotify cover three times). The copies are removed by the weekly cache cleanup.

- **Playlist covers you choose** are kept with your library data instead of in the cache folder, so clearing the cache no longer loses them. Covers chosen before are moved there on the first start.

## [0.5.1] — 2026-11-11

The library database is unchanged; 0.5.0 still opens it.

### Added

- **Loading from Spotify** shows: a small spinner in the header bar while searches, playlists,
  artists or albums are on their way.
- Shuffle and Repeat have a tooltip that names the current mode, such as "Shuffle: Albums" or
  "Repeat: One Song".
- A hidden extra for everyone who misses their CD shelf.

### Changed

- **The visualizer is off by default.** Turn it on in Preferences → Playback.
- The visualizer moves smoothly: its bars follow the music 50 times a second instead of 25 and
  rise over a few frames instead of jumping.
- Tooltips appear after GTK's usual short pause again instead of waiting a second.
- The development build and the test Flatpak have their own icon, with GNOME's yellow and black
  construction stripes, so they can be told apart from the installed Jewelcase.

### Fixed

- **Editing tags with cdport works with cdport 1.0.0**: Jewelcase no longer asks for "cdport 1.4.0
  or newer", and it offers to set up cdport's and lyrembed's D-Bus services again.

## [0.5.0] — 2026-11-11

A pre-release. Jewelcase is a music player for GNOME built around your own album library, with
Spotify woven in: the songs you own play from your files, also inside Spotify playlists, and the
songs you don't have stream through Spotify Soloist in the same queue. Without Spotify, it is a
complete local player.

### Features

- **Albums.** A grid of covers in a size you choose; editions of one album (deluxe, remastered,
  a Japanese version) are merged, with the edition as a subtitle. An album opens beside the grid,
  and each song unfolds to show its details.
- **Artists.** Album artists only, so featured guests don't clutter the list. Artist pages group
  albums, EPs & singles, soundtracks and collaborations, list the artist's songs on other albums,
  and show portraits, a short bio and links from online sources if you allow it.
- **Player.** A cover-coloured full player with synced lyrics, which say where they come from,
  and a visualizer. Korean, Japanese and Chinese lyrics show with a romanization. Gapless
  playback, ReplayGain, loudness levelling for untagged songs, and crossfade that follows where
  songs really fade. Media keys and GNOME's media controls work.
- **Queue and smart shuffle.** *Play Next*, *Add to Queue*, a reorderable queue that is kept
  between starts, and Recently Played. Shuffle songs or albums, or mix in similar songs by genre,
  tempo, artist or sound.
- **Playlists.** Mosaic covers, drag and drop, sorting like on Spotify, a search inside each
  playlist that also adds songs from your library or Spotify, and M3U8 and XSPF import and export.
- **Search.** One search (Ctrl+F) finds artists, albums, songs and playlists, your library's
  first and then Spotify's.
- **Listening statistics.** Top artists, albums, songs and genres of the last week, month, year
  or all time, with listening time and how each moved. Kept on this computer only.
- **Spotify playlists, both ways.** Your Spotify playlists show next to your own, with the songs
  you have playing from your files. Edit them from Jewelcase (songs, order, name, cover), publish
  a local playlist to Spotify, or remove one from Spotify.
- **Spotify in the library.** Saved albums and followed artists that aren't in your library get
  their own section; artist pages list the releases on Spotify that you don't have. Save albums
  and follow artists from Jewelcase.
- **Streaming through Spotify Soloist.** Songs you don't have play through Spotify's official
  player, in Jewelcase's queue; Preferences walks you through setting it up, with soloistd or
  without it. Spotify Connect devices can be controlled from the player.
- **Lyrics for Spotify songs** from LRCLIB, only while the lyrics are on screen.
- **Your files stay yours.** Jewelcase never writes to your music files. lyrembed finds and
  romanizes lyrics, cdport imports CDs and edits tags (Ear Tag, Kid3 or Picard can be chosen
  instead), and Jewelcase picks up their changes. Preferences → Integrations shows the steps to
  install and set them up.
- **Fits other desktops.** A light or dark choice, its own icons, and Spotify logins kept without
  a keyring.
- **Updates itself.** A banner names a new Jewelcase version, Soloist build or soloistd release
  and installs it only when you choose to.
- **Install** on Fedora 45: download the RPM from the release and run
  `sudo dnf install ./jewelcase-0.5.0-1.fc45.x86_64.rpm`.

[Unreleased]: https://github.com/sobakah/jewelcase/compare/0.10.0...HEAD
[0.10.0]: https://github.com/sobakah/jewelcase/releases/tag/0.10.0
[0.9.0]: https://github.com/sobakah/jewelcase/releases/tag/0.9.0
[0.8.0]: https://github.com/sobakah/jewelcase/releases/tag/0.8.0
[0.7.0]: https://github.com/sobakah/jewelcase/releases/tag/0.7.0
[0.6.1]: https://github.com/sobakah/jewelcase/releases/tag/0.6.1
[0.6.0]: https://github.com/sobakah/jewelcase/releases/tag/0.6.0
[0.5.1]: https://github.com/sobakah/jewelcase/releases/tag/0.5.1
[0.5.0]: https://github.com/sobakah/jewelcase/releases/tag/0.5.0
