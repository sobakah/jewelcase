# Changelog

## Unreleased

- Covers in the album, artist and playlist grids and on artist pages keep their size when the window or a panel changes width; only the number of columns changes. Their size is set in Preferences → Library → Appearance → Cover Size (120–280 px).
- Preferences → Library → Playlists: how songs are listed in playlists whose sort you haven't changed, separately for Gatefold's and Spotify's playlists (for example newest first). A sort chosen in a playlist stays; its sort menu's *Use Default Sort* goes back to the default.
- Development builds (`make run`, the test Flatpak) show their commit in the version, with "-dirty" for uncommitted changes; clicking the version in About copies it.
- The test Flatpak streams through its own Soloist: the one soloistd keeps up to date, run inside the sandbox with the Flatpak's own Soloist API key and data, as the Spotify device "Gatefold Devel". soloistd itself stays with the installed app (Preferences no longer shows its controls in a Flatpak).

## 0.3.0 (pre-release)

A new icon, side panels that slide like a record sleeve, cdimport's new name cdport, and both companion tools over D-Bus, with steps in Preferences to install and set them up. Gatefold also fits better on other desktops: a light or dark choice, its own icons, and Spotify logins kept without a keyring. The database doesn't change: 0.2.3 can still open it. 0.2.3 offers this update itself (its banner, then Install).

- A new icon: a gatefold LP, its cover slightly opened and the record halfway out.
- The side panels (album, playlist, artist page, and an album on an artist page) slide in like a record leaving its sleeve: a little slower, with a slight bounce and a shadow. The album panel on an artist page now reaches up to the top bar. When a panel closes, the albums behind it take the space right away instead of leaving a grey strip. The player's handle on top can be clicked to close it, like the panels' handles.
- Behind the top of the album, playlist and artist panels, the cover is now blurred as in the player (as large as the window, softer), so it reads as colour rather than as a second copy of the picture.
- The mini player's controls stay on the right also with the visualizer off.
- cdimport is now called **cdport**. From cdport 1.3.0 and lyrembed 1.2.0 on, Gatefold talks to them over D-Bus once their service is set up (`cdport --install-dbus-service`, `lyrembed --install-dbus-service`, or *Set Up* in Preferences). CD imports, lyrembed's pages and lyrics searches then keep running when Gatefold closes, and a Gatefold started again shows them. Without the service, Gatefold starts them directly, as before; cdimport 1.2.x still works.
- Preferences → Integrations shows, for lyrembed and cdport, what's missing and the steps to install, update or set them up, with commands to copy. A tool that's needed but not set up offers *How to Set Up*.
- Preferences → Library → Appearance → Style: Follow System, Light or Dark. On Xfce, MATE, Cinnamon and window managers, Follow System now also follows a dark GTK theme.
- Gatefold brings every icon it uses, so icon themes that lack some (such as KDE's Breeze) no longer show missing icons.
- Without a keyring service, Spotify logins are kept in a file only you can read instead of being lost at every restart; Preferences → Spotify says so.
- Spotify: artists' releases are asked for in fewer requests and kept for a week, which helps against Spotify's daily limit. When Spotify blocks those requests, the artist page says so also for artists in your library.
- For testing: `make flatpak` builds a Flatpak of Gatefold (see `build-aux/flatpak/README.md`).

### Install

Download `gatefold-0.3.0-1.fc45.x86_64.rpm` below and install it, or use the update notice in Gatefold 0.2.3:

```sh
sudo dnf install ./gatefold-0.3.0-1.fc45.x86_64.rpm
```

## 0.2.3 (pre-release)

A fix for Spotify songs played through Soloist, saving albums and following artists on Spotify, and songs without a stored length. Gatefold's database is updated on the first start; after that, 0.2.2 can't open it any more. 0.2.2 offers this update itself (its banner, then Install).

- Fixed: at the end of a Spotify song played through Soloist, Gatefold could race through the rest of the album in a second and stop at its start. That happened when Soloist first played a song left in its queue from earlier: the next song's own start was then taken for that left-over playing again. Should songs ever keep ending right after they start, Gatefold now stops and says so. When Gatefold's Next makes Soloist skip and it lands on a song left in its queue, Gatefold plays its own song directly.
- Save Spotify albums to your Spotify library from their panel (the + button next to Shuffle; a check mark once saved, click again to remove), and follow or unfollow artists on Spotify from their page (Follow next to their links, also for artists in your library). Gatefold asks Spotify when a panel or page opens, so changes made in the Spotify app show. A notice confirms it, with Undo. Following needs one more permission: press Connect Again in Preferences → Spotify (or on the notice Gatefold shows when Spotify refuses), and allow it in the browser.
- Preferences → Spotify has Connect Again next to Disconnect: a new login for new permissions, while your playlists stay. If the new login is cancelled, the current one stays in use; with a different account, the other account's playlists are removed as when disconnecting.
- Gatefold now writes what it decides about Soloist (which song plays, why one ended) to the system log (`journalctl --user -t gatefold`).
- Songs whose FLAC file doesn't say how long it is (some encoders leave that out) now get their length, so their progress bar, total time and seeking work. Such songs already in the library are read again once, on the next start. Until a song has a length, its bar stays empty instead of full.
- On a Spotify album, the artist's name opens their page: the one in your library if you have them, otherwise their Spotify page (before, only artists in your library were links).
- Clicking an album's title copies it (instead of putting a text cursor in it).

### Install

Download `gatefold-0.2.3-1.fc45.x86_64.rpm` below and install it, or use the update notice in Gatefold 0.2.2:

```sh
sudo dnf install ./gatefold-0.2.3-1.fc45.x86_64.rpm
```

## 0.2.2 (pre-release)

Listening statistics, a visualizer, and update notices. Gatefold's database is updated on the first start; after that, 0.2.1 can't open it any more. From this version on, Gatefold tells you about new versions itself and can install them; while the repository is private, it asks GitHub through the `gh` command, which must be logged in (`gh auth status`).

- Listening Statistics (main menu): what played most in the last week, month, year or ever, as top artists, albums, songs and genres, with plays, listening time as bars, and how each entry moved against the period before (↑, ↓, New). Artists and albums open in the library, songs play, and the top songs can become the queue. Plays are logged on this computer only, starting with Recently Played; Preferences → Playback can turn it off or delete it.
- A visualizer: bars in the cover color that move with songs from your library, between the cover and the title in the player, and next to the song in the mini player (more of them in a wide window). Spotify songs show a still strip. Preferences → Playback can turn it off; it's also off while GNOME's animations are off.
- Update notices, when Gatefold starts and twice a day: a banner names a new Gatefold version, a new Soloist build or a new soloistd release. Its dialog shows what's new and installs it only when you choose to: Gatefold's package is checked against GitHub's checksum and installed after GNOME asks for your password, then Gatefold offers to restart; Soloist is updated through soloistd. Preferences → Online can turn it off.
- Next in the Spotify app now skips to Gatefold's next song while a Spotify song plays through Soloist (when the next song is a Spotify song too).
- The Pre-Amp setting is gone: it stays at +4 dB, where your files and Spotify songs play equally loud. "Volume of Spotify Songs" is still there for fine-tuning.

### Install

Download `gatefold-0.2.2-1.fc45.x86_64.rpm` below and install it:

```sh
sudo dnf install ./gatefold-0.2.2-1.fc45.x86_64.rpm
```

## 0.2.1 (pre-release)

Recently played Spotify songs, and adding the song that plays to a playlist. Gatefold's database is updated on the first start; after that, 0.2.0 can't open it any more. If lyrics of Spotify songs aren't romanized, update lyrembed to 1.1.1 (`pipx upgrade lyrembed`) and restart Gatefold: Preferences → Integrations shows whether romanization is available.

- Recently Played (above the queue) now has the Spotify songs that played in Gatefold too, kept only on this computer. Each song has a menu (⋮ or right-click) to play it next, add it back to the queue or add it to a playlist.
- Songs in the queue, the one playing too, have "Add to Playlist…" in their right-click menu.
- The player has a + button next to the song that plays, for "Add to Playlist…". Its tooltip, Song Info and the song details in album and playlist panels name the playlists that already have the song, local or Spotify ones (also as its Spotify version).
- "Add to Playlist…" lists the playlists that have the song first, with a check mark; choosing one of them again removes the song from it. For several songs, a playlist that has some of them gets only the missing ones.

### Install

Download `gatefold-0.2.1-1.fc45.x86_64.rpm` below and install it:

```sh
sudo dnf install ./gatefold-0.2.1-1.fc45.x86_64.rpm
```

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
