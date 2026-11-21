<p align="center"><img src="data/icons/hicolor/scalable/apps/io.github.sobakah.Jewelcase.svg" width="128" alt=""></p>

# Jewelcase

A music player for GNOME, for people who keep their own music as albums and use Spotify as well.

Jewelcase is built around your local library: covers to browse, albums and artists that are tidy, lyrics, gapless playback. Spotify fills the gaps. Its playlists, saved albums and artists sit next to your own, and the songs you don't have play through Spotify in the same queue. Without Spotify, Jewelcase is a complete local player.

## Your library and Spotify together

- **What you own plays from your files**, also when it's part of a Spotify playlist: gapless, with ReplayGain and the lyrics in your files. Jewelcase matches Spotify songs to your files by ISRC, title, artist and length.
- **What you don't own comes from Spotify**: in playlists, in search results (after your library's) and on artist pages ("On Spotify": the releases you don't have). It streams through Spotify Soloist, Spotify's official player, in Jewelcase's queue.
- **Playlists go both ways.** Your Spotify playlists show next to your own; edit them from here (songs, order, name, cover). Publish a local playlist to Spotify. Add songs from your library or from Spotify to any playlist.
- **Saved albums and followed artists** that aren't in your library get their own section in the album and artist grids.
- **Popular songs** on artist pages: the artist's songs you played most, and optionally their most popular songs on Deezer beside them (Preferences → Library → *Popular Songs*); songs you don't have play from Spotify.
- **New releases** of the artists you follow, from the last 30 days, are listed under the bell in the header bar; a dot on it means something new. Jewelcase reads them from Deezer once a day, as Spotify only allows a few artist requests a day; clicking one opens it from Spotify. Preferences → Online → *New Releases of Followed Artists* (needs *Look Up Artists Online*).
- **Play on your Spotify Connect devices**, with the player showing what they play.

## Features

- **Albums**: a grid of covers, sorted by artist, title, year or date added. Editions (deluxe, remastered, a Japanese version) are merged into one album. An album opens beside the grid, and each song unfolds to show its details.
- **Artists**: album artists only, so featured guests don't clutter the list; sorted by name or date added. Artist pages group albums, EPs & singles, soundtracks and collaborations, and list the artist's songs on other albums. Portraits, a short bio and links come from online sources if you allow it.
- **Player**: a cover-coloured full player with synced lyrics. Korean, Japanese and Chinese lyrics show with a romanization. Crossfade follows where songs really fade, and albums stay gapless. Media keys and GNOME's media controls work.
- **Queue and smart shuffle**: *Play Next*, *Add to Queue*, a reorderable queue that's kept between starts. Shuffle songs or albums, or mix in similar songs by genre, tempo, artist or sound.
- **Playlists and search**: mosaic covers, drag and drop, search within a playlist, import and export (M3U8, XSPF). *Only Songs in My Library* in a playlist's sort menu hides the songs that would come from Spotify. One search (Ctrl+F) finds artists, albums, songs and playlists.
- **Listening statistics**: your top artists, albums, songs and genres of the last week, month, year or all time. They stay on your computer; Preferences → Playback turns them off or deletes them.
- **Your own icon**: Preferences → Library → *App Icon* puts another disc in the icon's jewel case. It's saved in `~/.local/share/icons`, which the dash and the app grid read first.
- **Your files stay yours**: Jewelcase never writes to your music files. Separate programs do, and Jewelcase picks up their changes (see below).

## Install

Jewelcase is in pre-release, for Fedora 45. Download the RPM from the [releases page](https://github.com/sobakah/jewelcase/releases) and install it:

```sh
sudo dnf install ./jewelcase-*.x86_64.rpm
```

Jewelcase tells you when a new version is out and installs it for you (Preferences → Online → *Check for Updates*).

Your library is kept in `~/.local/share/io.github.sobakah.Jewelcase/`, and cover thumbnails in `~/.cache/io.github.sobakah.Jewelcase/`.

### Fedora Silverblue, Bluefin and other image-based systems

These take Jewelcase as a Flatpak. Download `jewelcase-*.flatpak` from the [releases page](https://github.com/sobakah/jewelcase/releases) and install it; it fetches the GNOME runtime from Flathub:

```sh
flatpak remote-add --user --if-not-exists flathub https://dl.flathub.org/repo/flathub.flatpakrepo
flatpak install --user ./jewelcase-*.flatpak
```

- **Updates**: Jewelcase tells you when a new version is out; download its `.flatpak` file from the release page and install it the same way.
- **Your library** is read from your music folder; the Flatpak keeps its own database, settings and Spotify login in `~/.var/app/io.github.sobakah.Jewelcase/`.
- **lyrembed and cdport** work through their D-Bus services. Preferences → Integrations has the steps for these systems: lyrembed on the computer itself, cdport in a toolbox, where it can build cyanrip.
- **Ear Tag** can't be started from the Flatpak; cdport's own tag editor can.

## Connecting Spotify

You need Spotify Premium and a Spotify developer app of your own. Preferences → Spotify walks you through it:

1. Create an app at <https://developer.spotify.com/dashboard> (any name) and choose "Web API".
2. Add the redirect URI `http://127.0.0.1:8974/callback`, exactly as written.
3. Paste the app's client ID into Preferences and press *Connect*.

The login is kept in your keyring, or, without one, in a file only you can read.

**To stream songs you don't own**, install [Spotify Soloist](https://developer.spotify.com/documentation/soloist/reference/downloads-and-updates) and get a free Soloist API key. Preferences → Spotify → Streaming has the steps, with or without [soloistd](https://github.com/MopigamesYT/soloistd), which installs Soloist, keeps it updated and runs it in the background; Jewelcase then uses it, and Preferences can control it.

The first time, link Soloist by choosing it in the Spotify app's device list.

## Working with lyrembed and cdport

- [lyrembed](https://github.com/sobakah/lyrembed) finds lyrics, lets you check and edit them, and romanizes them; Jewelcase also uses it to romanize lyrics for display without changing the files.
- [cdport](https://github.com/sobakah/cdport) imports CDs (Jewelcase shows a banner when one is inserted) and edits tags, one album at a time, in your browser. [Ear Tag](https://flathub.org/apps/app.drey.EarTag), Kid3, Picard or a command of your own can edit tags instead (Preferences → Integrations → Tag Editor).

Preferences → Integrations shows whether they're found and the steps to install them and set up their D-Bus services. Through D-Bus, their pages and imports keep running when Jewelcase closes.

## Limits

- An RPM for Fedora 45, and a Flatpak for image-based systems; English only so far.
- No gapless playback or crossfade between a song from your files and one from Spotify.
- The songs of Spotify playlists you follow but don't own can't be listed (Spotify doesn't allow it); they can be played through Soloist.
- Spotify limits how often artists' releases can be read, per account (all apps on it share the allowance); an artist page reads their first 10 releases (*Show All* reads the rest), Jewelcase keeps them for a week and says when Spotify asks it to wait.

## License

Jewelcase's code is GPL-3.0-or-later. It uses bliss-audio for smart shuffle, which is GPL-3.0-only, so the program as built is GPL-3.0-only.
