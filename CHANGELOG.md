# Changelog

## Unreleased

- Playlists can be sorted like on Spotify: by title, artist, album, date added or duration, or in their own order, each also reversed. The choice is kept per playlist.
- A playlist's songs have the same actions as an album's: Add to Queue, Play Next, Add to Playlist, Go to Album and Go to Artist; the playlist's menu adds all of it to the queue or another playlist.
- The artist and album of a Spotify song in the player open their pages, like for your own songs.
- Preferences → Library → Start: open with the last view or a fixed one. Preferences → Playback → Player: whether lyrics and queue are shown when Gatefold starts.
- Artist pages cover less of the artist grid. Side panels (albums, playlists, artists) close with a grip on their left edge, like the one on top of the player, instead of a close button.
- Album, playlist and artist panels show their cover blurred behind the top, like the player.
- Artist photos come from Spotify (while it's connected) and Deezer, then Wikimedia Commons; fanart.tv is gone, and its API key with it. Artists looked up before get their photos looked up once more.
- Fixed: searching Spotify always failed with "403: Insufficient client scope". Matching a playlist's songs then asked Spotify once for every song, again at every sync, which could get Gatefold blocked for hours ("too many requests"). Searches now stop at the first error, and while Spotify asks to wait for a long time, Gatefold doesn't ask again, also after a restart.
- When Spotify doesn't show an artist's releases, the page now says why.

## 0.1.0 (pre-release)

The first public version of Gatefold, a local-first music player for GNOME built around albums, artists and playlists. It's a pre-release: everything described here works, but expect rough edges, and keep backups of playlists you care about (“Export…” in a playlist’s menu).

### Library

- Scans your music folders and groups the songs into albums. Editions of one album (deluxe, remastered, Japanese versions) are merged, with the edition as a subtitle; artist pages sort them into albums, EPs & singles and soundtracks.
- Covers come from the files or the album folder, and are what you browse by: the album grid fills the window at any width.
- Folders that are temporarily missing (an unmounted drive, a NAS that's off) keep their songs.
- Changes made by other programs show up live.

### Artists

- An artist list with album artists only, so featured guests don't clutter it.
- Artist pages with the albums grouped by type, collaborations, and "Appears on" for their songs on other artists' albums.
- Optional online details: portraits from fanart.tv and Wikimedia Commons, a short Wikipedia bio, and links to Wikipedia, MusicBrainz and Spotify. Off until you agree at first start. Portraits can be replaced.

### Player

- A full-window player with the cover as large as possible, coloured by the cover, plus lyrics and queue panes that open next to it on wide windows.
- Synced lyrics from the files or `.lrc` files, with an original line and its romanization (Korean, Japanese, Chinese); missing romanization is added automatically through lyrembed.
- Gapless playback, ReplayGain, and loudness levelling for songs without ReplayGain tags (measured by the song analysis).
- Crossfade (0–12 s) that follows where songs actually fade out and start; albums played in order stay gapless.
- Media keys and GNOME's media controls (MPRIS).

### Queue and shuffle

- "Play next" and "Add to queue" go before the rest of the album or playlist; the queue can be reordered, cleared and saved as a playlist, and is restored when Gatefold starts.
- Shuffle songs or whole albums, repeat, and **smart shuffle**: mix in similar songs by genre, tempo, artist or sound, and keep playing similar music when the queue ends. The song analysis behind it runs in the background and is off until turned on.

### Playlists and search

- Playlists with mosaic covers, drag and drop, and M3U8/XSPF import and export.
- One search for artists, albums, songs and playlists, in a card over the current view.

### Companion tools

- **lyrembed**: find lyrics automatically, or check and edit them in its page.
- **cdimport**: import a CD, with a banner when one is inserted.
- **Ear Tag** (or Kid3, Picard, a command of your own): edit tags and covers; Gatefold itself never writes to your files.

### Spotify (optional)

Needs Spotify Premium and a Spotify developer app of your own (Preferences → Spotify walks through it).

- Control Spotify Connect devices from the player.
- Your Spotify playlists next to the local ones; songs you have play from your files. Your own playlists can be edited, and local ones published to Spotify.
- Saved albums and followed artists in their own section of the album and artist grids, and the releases of an artist on Spotify that you don't have.
- Streaming of songs you don't have through Spotify Soloist, Spotify's official headless player (installed separately; soloistd can install and update it and is managed from Preferences).

### Known limitations

- Fedora 45 only, as an RPM; English only so far.
- No gapless or crossfade between a local and a Spotify song.
- The songs of followed Spotify playlists can't be listed (Spotify doesn't allow it); they can be played through Soloist.
- The RPM doesn't include librespot streaming; build with `--with spotify_stream` for it.
- Known issues from the code review are listed in CONCEPT.md ("Known issues").

### Install

Download `gatefold-0.1.0-1.fc45.x86_64.rpm` below and install it:

```sh
sudo dnf install ./gatefold-0.1.0-1.fc45.x86_64.rpm
```
