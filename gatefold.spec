# Local RPM for Fedora 45 (CONCEPT.md §12). Build with `make rpm` or `make mock`.
#
# rpmbuild --with spotify_stream enables librespot streaming (CONCEPT.md §9.4).
%bcond spotify_stream 0

%global app_id io.github.sobakah.Gatefold

Name:           gatefold
Version:        0.2.3
Release:        1%{?dist}
Summary:        Album-focused local music player

# Gatefold's own code is GPL-3.0-or-later; bliss-audio (smart shuffle) is
# GPL-3.0-only, so the built program is GPL-3.0-only. The vendored crates'
# licenses are listed in LICENSE.dependencies; add them here before
# publishing this anywhere. The bundled Adwaita icons are
# LGPL-3.0-only OR CC-BY-SA-3.0 (data/resources/icons/README.md).
License:        GPL-3.0-only AND (LGPL-3.0-only OR CC-BY-SA-3.0)
URL:            https://github.com/sobakah/gatefold
Source0:        %{name}-%{version}.tar.xz
# Rust dependencies: `cargo vendor` output, created by `make tarballs`.
Source1:        %{name}-%{version}-vendor.tar.xz

ExclusiveArch:  %{rust_arches}

BuildRequires:  cargo-rpm-macros >= 26
BuildRequires:  rust >= 1.95
BuildRequires:  cargo
BuildRequires:  meson >= 1.4
BuildRequires:  blueprint-compiler
BuildRequires:  pkgconfig(gtk4) >= 4.24
BuildRequires:  pkgconfig(libadwaita-1) >= 1.10
BuildRequires:  pkgconfig(gstreamer-1.0) >= 1.28
BuildRequires:  pkgconfig(gstreamer-app-1.0) >= 1.28
BuildRequires:  pkgconfig(sqlite3)
# glycin (sandboxed cover decoding) links these.
BuildRequires:  pkgconfig(fontconfig)
BuildRequires:  pkgconfig(libseccomp)
BuildRequires:  /usr/bin/glib-compile-resources
BuildRequires:  /usr/bin/glib-compile-schemas
BuildRequires:  /usr/bin/xmllint
BuildRequires:  gettext
BuildRequires:  desktop-file-utils
BuildRequires:  /usr/bin/appstreamcli

# Playback (CONCEPT.md §12): playbin3, FLAC and ReplayGain, AAC/ALAC in M4A.
Requires:       gstreamer1-plugins-base
Requires:       gstreamer1-plugins-good
Requires:       gstreamer1-plugin-libav
# Song analysis (smart shuffle, crossfade, loudness): bpmdetect (soundtouch).
Requires:       gstreamer1-plugins-bad-free
# Cover decoding in glycin's sandbox.
Requires:       glycin-loaders
Requires:       bubblewrap
Recommends:     gstreamer1-plugins-ugly-free
# Ear Tag, the default tag editor, is a Flatpak (CONCEPT.md §10.3).
Recommends:     flatpak

%description
Gatefold is a local-first music player for people who listen to albums,
follow artists and curate playlists. Cover art is the main way around the
library; lyrics, the queue and details open when you want them.

%prep
%autosetup -p1 -a1
%cargo_prep -v vendor

%build
%meson -Dprofile=default -Dspotify_stream=%{?with_spotify_stream:true}%{!?with_spotify_stream:false}
%meson_build
%if %{with spotify_stream}
%{cargo_license_summary -f spotify-stream}
%{cargo_license -f spotify-stream} > LICENSE.dependencies
%else
%{cargo_license_summary}
%{cargo_license} > LICENSE.dependencies
%endif
%{cargo_vendor_manifest}

%install
%meson_install
# Once po/LINGUAS lists a language: add `%%find_lang %%{name}` here and
# `-f %%{name}.lang` to %%files (find_lang fails while there are none).

%check
desktop-file-validate %{buildroot}%{_datadir}/applications/%{app_id}.desktop
appstreamcli validate --no-net %{buildroot}%{_metainfodir}/%{app_id}.metainfo.xml
%meson_test

%files
%license LICENSE LICENSE.dependencies cargo-vendor.txt
%doc README.md
%{_bindir}/%{name}
%{_datadir}/applications/%{app_id}.desktop
%{_metainfodir}/%{app_id}.metainfo.xml
%{_datadir}/glib-2.0/schemas/%{app_id}.gschema.xml
%{_datadir}/icons/hicolor/scalable/apps/%{app_id}.svg
%{_datadir}/icons/hicolor/symbolic/apps/%{app_id}-symbolic.svg

%changelog
* Sun Nov 08 2026 sobakah - 0.2.3-1
- Fixed racing through an album at the end of a Spotify song played
  through Soloist; Soloist's decisions go to the system log
- Save Spotify albums and follow artists on Spotify, with Undo;
  Connect Again in Preferences for new permissions
- Songs whose FLAC header lacks the length get it (read again once)
- Spotify album artists link to their Spotify page; a click on an
  album title copies it

* Sun Nov 08 2026 sobakah - 0.2.2-1
- Listening statistics: top artists, albums, songs and genres of the
  week, month, year or all time, with listening time and trends
- A visualizer for songs from the library, in the player and the mini
  player
- Update notices for Gatefold, Soloist and soloistd, installed only
  when asked
- Next in the Spotify app skips to Gatefold's next song while Soloist
  plays; the Pre-Amp setting is gone (fixed at +4 dB)

* Sun Nov 08 2026 sobakah - 0.2.1-1
- Recently Played has the Spotify songs that played in Gatefold, with a
  menu to queue them again or add them to a playlist
- "Add to Playlist…" for the playing song (+ next to its title, and in
  the queue); playlists that have the song come first and choosing one
  again removes it; Song Info lists them

* Sun Nov 08 2026 sobakah - 0.2.0-1
- Spotify playlists: reorder and rename from the menu, covers, delete
  or unfollow; safer sending (placed by song, no doubles, locked while
  sending); ISRCs from MusicBrainz for matching
- Search: results by kind with Spotify's, an icon for library or
  Spotify, recent searches
- Lyrics for Spotify songs from LRCLIB; romanization for display through
  lyrembed 1.1.1
- Soloist: the Spotify app keeps control of songs chosen there; many
  fixes; the API key goes into a file once Soloist allows it
- Library: moved folders keep their songs' history; ctime; periodic
  checks; scans write only what changed
- Related artists from ListenBrainz for smart shuffle; resizable artist
  pages; keyboard access to panel widths

* Sun Nov 08 2026 sobakah - 0.1.1-1
- Playlists sort like on Spotify; playlist songs get the album songs'
  actions; Spotify songs' artist and album open from the player
- Start options for the view and the player's panes
- Side panels: cover backdrop, a grip to close them
- Artist photos from Spotify and Deezer instead of fanart.tv
- Fixed Spotify search (403), and the request flood it caused; rate
  limits are kept across restarts

* Fri Nov 06 2026 sobakah - 0.1.0-1
- Project skeleton (roadmap phase 0)
- Library scanner, album grid and panel, playback, queue, MPRIS (phase 1)
- Artists: grid, pages, online lookups, portraits (phase 2)
- Player: cover colors, adaptive layouts, lyrics, queue sections (phase 3)
- Companion tools: lyrembed, cdimport, tag editor (phase 4)
- Playlists, M3U8/XSPF import and export, search (phase 5)
- Smart shuffle: song analysis, similar songs, album shuffle (phase 6)
- Spotify account: Connect devices, playlists matched to the library,
  two-way editing, "Create on Spotify" (phase 7)
- Streaming Spotify songs through Spotify Soloist (official, installed
  separately), or librespot with --with spotify_stream (phase 8)
