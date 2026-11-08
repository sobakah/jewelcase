# Changelog

## 0.2.0 (pre-release)

Spotify playlists you can fully edit, a new search, lyrics for Spotify songs, and a round of fixes from a review of the whole program. Gatefold's database is updated on the first start; after that, 0.1.1 can't open it any more. Automatic romanization needs lyrembed 1.1.1 (click **Update** on lyrembed's page, or `pipx upgrade lyrembed`). You may be asked to connect to Spotify again once, for the permission to delete or unfollow playlists.

### Spotify playlists

- Your own Spotify playlists can be reordered and renamed from Gatefold, on purpose a step away from the local ones: "Change Order on Spotify" in the playlist's menu makes the songs draggable until you press Done, and "Rename on Spotify…" asks for the new name.
- A new cover for one of your synced playlists goes to Spotify too.
- Spotify playlists show Spotify's cover, also when some of their songs are in your library (e.g. one album's cover showed instead).
- Changes wait 4 seconds after your last edit, then go out together. While they're on their way, the playlist can't be changed ("sending changes to Spotify…").
- Songs added or moved while someone changed the playlist elsewhere (on the phone) land after the right song, not at a shifted position. A song whose confirmation got lost (a timeout) is no longer added twice.
- "Create on Spotify" clicked twice creates one playlist.
- "Remove from Spotify…" deletes your own playlist on Spotify (Spotify keeps it restorable for 90 days) or unfollows someone else's; it can stay in Gatefold as a local playlist. You may be asked to connect to Spotify again once, for the extra permission.
- With online lookups on, songs without an ISRC get the ones MusicBrainz knows (one request per album, once), so more Spotify playlist songs are found in your library.

### Lyrics

- Spotify songs that aren't in your library get lyrics from LRCLIB, synced where LRCLIB has them. Gatefold asks only for the song that plays, only while the lyrics are shown, and remembers every answer. Preferences → Online → Lyrics turns it off.
- Korean, Japanese and Chinese lyrics without romanization get it under each line, like lyrics lyrembed romanized in the files, also lyrics from LRCLIB. This needs lyrembed 1.1.1 or newer and writes nothing to the files.

### Search

- Search results are sorted by kind: artists, albums, songs and playlists, each with your library first and then what Spotify has, and an icon on every row says whether it's in your library or only on Spotify. Spotify's artists and albums open in Gatefold; its playlists play with Soloist or open in Spotify.
- Spotify is asked once per search, not for every letter, and the same search isn't asked again while Gatefold runs.
- The search field shows your last five searches while it's empty.
- Names with "&" (such as "&TEAM") no longer cause GTK warnings in result lists.

### Soloist and Spotify Connect

- Picking or skipping songs in the Spotify app while Gatefold plays through Soloist works: the app's song plays and Gatefold shows it, instead of replacing it with its own next song.
- When the Spotify app moves playback away from Soloist, Gatefold stops showing Soloist's song.
- Shuffle or repeat failing no longer marks a song as unavailable; giving up on a song pauses Soloist, so it doesn't start late.
- A stopped soloistd service is no longer waited for; Gatefold starts its own Soloist. Turning streaming off while Gatefold connects really stops it.
- Moving playback to a device pauses a streamed song too. While Spotify can't be reached, streaming is no longer restarted every minute.

### Library and playback

- Renaming or moving a music folder keeps the songs' play history, queue and playlist entries, Spotify matches and "added" dates.
- Tag edits that keep the file's date (Picard's "preserve timestamps") are noticed. The library is also checked every 30 minutes, for changes on a network share.
- Smart shuffle's "same artist" also mixes in related artists (from ListenBrainz, with online lookups on).
- In shuffle, a song without ReplayGain tags after a tagged one no longer starts too loud or quiet for a moment.
- Playlists update only the rows that changed, and the cover cache no longer keeps old playlist mosaics and portrait choices forever.
- Large libraries: a scan that finds a new album writes only what changed, and the window no longer waits while the library is read again afterwards.
- Album, playlist and artist panels no longer look doubled while you drag their edge, and the grid behind follows the edge as you drag.
- Artist pages can be made wider or narrower by dragging their left edge, like album and playlist panels; the width is kept separately.
- The panel width can be changed with the keyboard (focus the panel's edge, then Left or Right), and the time next to the seek bar switches with Enter or Space.

### Install

Download `gatefold-0.2.0-1.fc45.x86_64.rpm` below and install it:

```sh
sudo dnf install ./gatefold-0.2.0-1.fc45.x86_64.rpm
```

## 0.1.1 (pre-release)

Mostly playlists, Spotify fixes and a new look for the side panels. Gatefold's database is updated on the first start; after that, 0.1.0 can't open it any more.

### Playlists

- Sort a playlist like on Spotify: by title, artist, album, date added or duration, or in its own order, each also reversed. It's kept per playlist and only changes the view (and what Play plays); dragging works in the playlist's own order.
- A playlist's songs have the same actions as an album's: Add to Queue on the row, and Play Next, Add to Playlist, Go to Album and Go to Artist in its menu, also for songs that are only on Spotify. The playlist's menu adds all of it to the queue or another playlist.

### Player and panels

- The artist and album of a Spotify song in the player open their pages: the library's when you have them, else Spotify's.
- Preferences → Library → Start: open with the last view or a fixed one. Preferences → Playback → Player: whether lyrics and queue are shown when Gatefold starts.
- Album, playlist and artist panels show their cover blurred behind the top, like the player, and close with a grip on their left edge instead of a close button. Artist pages cover less of the artist grid.

### Artist photos

- Photos come from Spotify (while it's connected) and Deezer, then Wikimedia Commons. fanart.tv is no longer used, so no API key is needed. Deezer finds artists by name and checks them against your albums, so an artist with the same name isn't mistaken for yours.
- Artists looked up before get their photos looked up once more. Photos you chose yourself stay.

### Spotify fixes

- Searching Spotify always failed with "403: Insufficient client scope"; it works again.
- Because of that, matching a playlist's songs asked Spotify once for every song, again at every sync, which could get Gatefold blocked for many hours ("too many requests"). Searches now stop at the first error, and while Spotify asks to wait, Gatefold doesn't ask again, also after a restart.
- When Spotify doesn't show an artist's releases, the page says why.

### Install

Download `gatefold-0.1.1-1.fc45.x86_64.rpm` below and install it:

```sh
sudo dnf install ./gatefold-0.1.1-1.fc45.x86_64.rpm
```

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
