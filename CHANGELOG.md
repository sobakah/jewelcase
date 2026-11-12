# Changelog

All notable changes to Jewelcase are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and versions follow
[Semantic Versioning](https://semver.org/).

## [Unreleased]

### Added

- **Sort artists** by name or by date added, from the sort button on the Artists page.

### Changed

- Albums saved on Spotify and artists followed there are sorted like your own albums and artists, instead of in the order you saved or followed them.

- **A new app icon**: a clear CD case with the disc inside, drawn flat like GNOME's own icons.

### Fixed

- The cover cache took about twice the space it needs: covers and artist pictures smaller than a thumbnail size were stored again at each bigger size (every Spotify cover three times). The copies are removed by the weekly cache cleanup.

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

[Unreleased]: https://github.com/sobakah/jewelcase/compare/0.5.1...HEAD
[0.5.1]: https://github.com/sobakah/jewelcase/releases/tag/0.5.1
[0.5.0]: https://github.com/sobakah/jewelcase/releases/tag/0.5.0
