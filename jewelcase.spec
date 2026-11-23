# Local RPM for Fedora 45 (CONCEPT.md §12). Build with `make rpm` or `make mock`.
#
# rpmbuild --with spotify_stream enables librespot streaming (CONCEPT.md §9.4).
%bcond spotify_stream 0

%global app_id io.github.sobakah.Jewelcase

Name:           jewelcase
Version:        0.13.1
Release:        1%{?dist}
Summary:        Album-focused local music player

# Jewelcase's own code is GPL-3.0-or-later; bliss-audio (smart shuffle) is
# GPL-3.0-only, so the built program is GPL-3.0-only. The vendored crates'
# licenses are listed in LICENSE.dependencies; add them here before
# publishing this anywhere. The bundled Adwaita icons are
# LGPL-3.0-only OR CC-BY-SA-3.0 (data/resources/icons/README.md).
License:        GPL-3.0-only AND (LGPL-3.0-only OR CC-BY-SA-3.0)
URL:            https://github.com/sobakah/jewelcase
# Gatefold is Jewelcase's old name: this package replaces it (Jewelcase
# moves Gatefold's library and settings over on its first start).
Provides:       gatefold = %{version}-%{release}
Obsoletes:      gatefold < %{version}-%{release}
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
# Ear Tag, a tag editor that can be chosen, is a Flatpak (CONCEPT.md §10.3).
Recommends:     flatpak

%description
Jewelcase is a local-first music player for people who listen to albums,
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
# Gatefold's updater restarts it as `gatefold` after installing an update;
# that start opens Jewelcase.
ln -s %{name} %{buildroot}%{_bindir}/gatefold
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
%{_bindir}/gatefold
%{_datadir}/applications/%{app_id}.desktop
%{_metainfodir}/%{app_id}.metainfo.xml
%{_datadir}/glib-2.0/schemas/%{app_id}.gschema.xml
%{_datadir}/icons/hicolor/scalable/apps/%{app_id}.svg
%{_datadir}/icons/hicolor/symbolic/apps/%{app_id}-symbolic.svg

%changelog
* Mon Nov 23 2026 sobakah - 0.13.1-1
- A phone over USB syncs with its newest sync folder
- Published playlists show Spotify's cover and show only once

* Mon Nov 23 2026 sobakah - 0.13.0-1
- Sync with Jewelcase Mobile through a shared folder or a USB-connected phone
- Choose which devices to sync with

* Sun Nov 22 2026 sobakah - 0.12.0-1
- Spotify songs match files by any of their ISRCs

* Sat Nov 21 2026 sobakah - 0.11.0-1
- Popular songs on artist pages: your most played, optionally Deezer's
- Every song in the queue has a menu with Go to Album and Go to Artist
- Add to Playlist opens as a menu and can be sorted
- The visualizer follows its setting again

* Sat Nov 21 2026 sobakah - 0.10.0-1
- Window layout keeps what was opened last in view as the window narrows
- Nothing is cut off at the window's edge at any width
- Playlists glide to their new places like albums and artists
- Artist pages read their first 10 releases from Spotify (Show All)

* Fri Nov 20 2026 sobakah - 0.9.0-1
- Large playlists scroll smoothly and open at once
- Albums glide to their places when a panel opens, closes or is resized
- Artist pages open beside the artist grid
- Lyrics of Spotify songs start at their first line
- Covers of new releases from the bell show

* Sat Nov 14 2026 sobakah - 0.8.0-1
- Choose the app icon's disc in Preferences
- A Flatpak bundle for image-based systems
- Next on a Spotify song no longer skips the song after it
- The mini player's visualizer is centred again

* Fri Nov 13 2026 sobakah - 0.7.0-1
- Spotify playlists can show only the songs in the library
- Spotify songs no longer jump to the next one after a seek
- The player closes again when clicking beside it; no × in the player
- New releases: albums named after a single, releases by two artists

* Thu Nov 12 2026 sobakah - 0.6.1-1
- App icon as large as other apps' icons
- Smaller Spotify icon; the artist lines up with the song title

* Thu Nov 12 2026 sobakah - 0.6.0-1
- New releases of followed Spotify artists under a bell (from Deezer)
- Artists sortable; Spotify's albums and artists sorted like the library
- New app icon; half the cover cache; chosen playlist covers kept

* Wed Nov 11 2026 sobakah - 0.5.1-1
- Editing tags works with cdport 1.0.0; D-Bus setup offered again
- Spinner while loading from Spotify; mode tooltips on Shuffle and Repeat
- Visualizer off by default and smoother; tooltips without the long wait

* Wed Nov 11 2026 sobakah - 0.5.0-1
- Pre-release: a GNOME music player for an album library with Spotify
  woven in (playlists both ways, saved albums, followed artists,
  streaming through Spotify Soloist); lyrics with romanization, smart
  shuffle, listening statistics; lyrembed and cdport as companion tools
