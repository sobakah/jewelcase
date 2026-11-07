# Local RPM for Fedora 45 (CONCEPT.md §12). Build with `make rpm` or `make mock`.
#
# rpmbuild --with spotify_stream enables librespot streaming (CONCEPT.md §9.4).
%bcond spotify_stream 0

%global app_id io.github.sobakah.Gatefold

Name:           gatefold
Version:        0.1.0
Release:        1%{?dist}
Summary:        Album-focused local music player

# Gatefold itself. The vendored crates' licenses are listed in
# LICENSE.dependencies; add them here before publishing this anywhere.
# Note: bliss-audio (smart shuffle) is GPL-3.0-only, so the built program
# as a whole is GPL-3.0 (without "or later").
License:        GPL-3.0-or-later
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
%{cargo_license_summary}
%{cargo_license} > LICENSE.dependencies
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
- Optional Spotify streaming through librespot, --with spotify_stream (phase 8)
