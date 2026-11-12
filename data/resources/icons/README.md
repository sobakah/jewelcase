# Icons

- `jewelcase-*-symbolic.svg`: Jewelcase's own icons (under the app's license),
  except `jewelcase-spotify-symbolic.svg`: Spotify's icon from Spotify's design
  guidelines (developer.spotify.com/documentation/design), a trademark of
  Spotify AB, used unchanged to attribute content from Spotify. Their rules:
  black on light backgrounds, white on dark ones (style.css: `.spotify-logo`),
  never smaller than 21 px. Jewelcase follows the colours; the size is
  smaller (16 px in lists and the mini player, `SPOTIFY_ICON_SIZE`; 20 px in
  the player), the owner's choice.
- All other files in `scalable/actions/` are copied unchanged from
  [adwaita-icon-theme](https://gitlab.gnome.org/GNOME/adwaita-icon-theme) 51,
  licensed LGPL-3.0-only OR CC-BY-SA-3.0. They are bundled so that icon themes
  without them (e.g. KDE's Breeze) don't show missing icons; a theme's own icon
  of the same name is still preferred.
