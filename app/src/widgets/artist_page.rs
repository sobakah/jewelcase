//! An artist's page (CONCEPT.md §4.2): portrait, counts, Play all, links,
//! a short bio, and the albums grouped by release type, plus
//! Collaborations and "Appears on".

use std::cell::{Cell, RefCell};

use adw::prelude::*;
use adw::subclass::prelude::*;
use gatefold_core::db::Album;
use gatefold_core::library::artists::AlbumRole;
use gatefold_core::library::covers::CoverSize;
use gatefold_core::library::titles::ReleaseType;
use gettextrs::{gettext, ngettext};
use gtk::{gio, glib};

use super::{AlbumPanel, AlbumTile, CoverImage};
use crate::application::GatefoldApplication;
use crate::objects::{AlbumObject, ArtistObject, TrackObject};
use crate::{covers, format};

/// The width of an album tile on artist pages, with its spacing.
const TILE_WIDTH: u32 = 198;

mod imp {
    use super::*;

    #[derive(Debug, Default, gtk::CompositeTemplate)]
    #[template(resource = "/io/github/sobakah/Gatefold/ui/artist_page.ui")]
    pub struct ArtistPage {
        #[template_child]
        pub identify_banner: TemplateChild<adw::Banner>,
        #[template_child]
        pub split_view: TemplateChild<adw::OverlaySplitView>,
        #[template_child]
        pub portrait_button: TemplateChild<gtk::MenuButton>,
        #[template_child]
        pub portrait: TemplateChild<CoverImage>,
        #[template_child]
        pub name: TemplateChild<gtk::Label>,
        #[template_child]
        pub latin_name: TemplateChild<gtk::Label>,
        #[template_child]
        pub counts: TemplateChild<gtk::Label>,
        #[template_child]
        pub links: TemplateChild<adw::WrapBox>,
        #[template_child]
        pub bio_box: TemplateChild<gtk::Box>,
        #[template_child]
        pub bio: TemplateChild<gtk::Label>,
        #[template_child]
        pub bio_toggle: TemplateChild<gtk::Button>,
        #[template_child]
        pub sections: TemplateChild<gtk::Box>,
        #[template_child]
        pub album_panel: TemplateChild<AlbumPanel>,
        #[template_child]
        pub actions: TemplateChild<adw::WrapBox>,
        /// A page for an artist only on Spotify (§9.8).
        pub spotify_id: RefCell<Option<String>>,
        /// Counts fills of the sections; an older "On Spotify" answer is
        /// dropped.
        pub generation: Cell<u64>,

        pub artist_id: Cell<i64>,
        pub artist: RefCell<Option<ArtistObject>>,
        /// The artist's own albums in page order, for Play All.
        pub own_albums: RefCell<Vec<AlbumObject>>,
        /// The album grids, which share one column count.
        pub flows: RefCell<Vec<gtk::FlowBox>>,
        pub columns: Cell<u32>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for ArtistPage {
        const NAME: &'static str = "GatefoldArtistPage";
        type Type = super::ArtistPage;
        type ParentType = adw::Bin;

        fn class_init(klass: &mut Self::Class) {
            AlbumPanel::ensure_type();
            CoverImage::ensure_type();
            klass.bind_template();
            klass.install_action("artist.close", None, |page, _, _| {
                page.emit_by_name::<()>("close", &[]);
            });
            klass.install_action("artist.play", None, |page, _, _| page.play_all(false));
            klass.install_action("artist.shuffle", None, |page, _, _| page.play_all(true));
            klass.install_action_async("artist.choose-image", None, |page, _, _| async move {
                page.choose_image().await;
            });
            klass.install_action("artist.choose-online", None, |page, _, _| {
                super::super::portrait_dialog::show(page, page.imp().artist_id.get());
            });
            klass.install_action("artist.reset-portrait", None, |page, _, _| {
                GatefoldApplication::get()
                    .library()
                    .choose_portrait(page.imp().artist_id.get(), None);
            });
            klass.install_action("artist.identify", None, |page, _, _| {
                if let Some(artist) = page.imp().artist.borrow().clone() {
                    super::super::identify_dialog::show(page, &artist);
                }
            });
            klass.install_action("artist.edit-tags", None, |page, _, _| {
                // All tracks of the artist's own albums, e.g. to set
                // ALBUMARTISTS for a real split (§3.3, §10.3).
                let library = GatefoldApplication::get().library();
                let files: Vec<std::path::PathBuf> = library
                    .artist_albums(page.artist_id())
                    .iter()
                    .filter(|(_, role)| *role == AlbumRole::Credit)
                    .flat_map(|(album, _)| library.album_tracks(album.id))
                    .map(|t| t.track().record.path.clone())
                    .collect();
                GatefoldApplication::get().tools().edit_tags(files);
            });
            klass.install_action("artist.lookup", None, |page, _, _| {
                GatefoldApplication::get()
                    .library()
                    .lookup_again(page.imp().artist_id.get(), None);
            });
        }

        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for ArtistPage {
        fn signals() -> &'static [glib::subclass::Signal] {
            static SIGNALS: std::sync::OnceLock<Vec<glib::subclass::Signal>> =
                std::sync::OnceLock::new();
            SIGNALS.get_or_init(|| vec![glib::subclass::Signal::builder("close").build()])
        }

        fn constructed(&self) {
            self.parent_constructed();
            let obj = self.obj();
            self.portrait
                .set_placeholder_icon("avatar-default-symbolic");
            obj.set_layout_manager(None::<gtk::LayoutManager>);
            let library = GatefoldApplication::get().library();
            library.connect_local(
                "artist-changed",
                false,
                glib::clone!(
                    #[weak]
                    obj,
                    #[upgrade_or]
                    None,
                    move |values| {
                        let id = values[1].get::<i64>().ok()?;
                        if id == -1 || id == obj.imp().artist_id.get() {
                            obj.refresh_header();
                        }
                        None
                    }
                ),
            );
            library.connect_local(
                "reloaded",
                false,
                glib::clone!(
                    #[weak]
                    obj,
                    #[upgrade_or]
                    None,
                    move |_| {
                        obj.refresh();
                        None
                    }
                ),
            );
            // Connected or not: the "On Spotify" part comes or goes.
            GatefoldApplication::get()
                .spotify()
                .connect_status_notify(glib::clone!(
                    #[weak]
                    obj,
                    move |_| {
                        if obj.spotify_artist_id().is_none() {
                            obj.fill_sections();
                        }
                    }
                ));
            self.album_panel.connect_local(
                "close",
                false,
                glib::clone!(
                    #[weak]
                    obj,
                    #[upgrade_or]
                    None,
                    move |_| {
                        obj.imp().split_view.set_show_sidebar(false);
                        None
                    }
                ),
            );
            self.bio_toggle.connect_clicked(glib::clone!(
                #[weak]
                obj,
                move |button| {
                    let bio = &obj.imp().bio;
                    let collapsed = bio.lines() > 0;
                    bio.set_lines(if collapsed { -1 } else { 2 });
                    bio.set_ellipsize(if collapsed {
                        gtk::pango::EllipsizeMode::None
                    } else {
                        gtk::pango::EllipsizeMode::End
                    });
                    button.set_label(&if collapsed {
                        gettext("Less")
                    } else {
                        gettext("More")
                    });
                }
            ));
        }
    }

    // The page lays out its child itself (no layout manager, see
    // constructed()) to learn its width.
    impl WidgetImpl for ArtistPage {
        fn request_mode(&self) -> gtk::SizeRequestMode {
            self.obj()
                .child()
                .map_or(gtk::SizeRequestMode::ConstantSize, |c| c.request_mode())
        }

        fn measure(&self, orientation: gtk::Orientation, for_size: i32) -> (i32, i32, i32, i32) {
            self.obj()
                .child()
                .map_or((0, 0, -1, -1), |c| c.measure(orientation, for_size))
        }

        fn size_allocate(&self, width: i32, height: i32, baseline: i32) {
            if let Some(child) = self.obj().child() {
                child.allocate(width, height, baseline, None);
            }
            // Same tile size in every section: as many columns as fit
            // tiles of about TILE_WIDTH, also for sections with few releases.
            let available = self.sections.width();
            if available <= 0 {
                return;
            }
            let columns = (available as u32 / TILE_WIDTH).clamp(2, 8);
            if self.columns.replace(columns) != columns {
                let obj = self.obj().downgrade();
                glib::idle_add_local_once(move || {
                    if let Some(obj) = obj.upgrade() {
                        obj.apply_columns();
                    }
                });
            }
        }
    }
    impl BinImpl for ArtistPage {}
}

glib::wrapper! {
    pub struct ArtistPage(ObjectSubclass<imp::ArtistPage>)
        @extends gtk::Widget, adw::Bin,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl ArtistPage {
    pub fn new(artist_id: i64) -> Self {
        let page: Self = glib::Object::new();
        page.imp().artist_id.set(artist_id);
        page.refresh();
        page
    }

    /// A page for an artist on Spotify that isn't in the library (§9.8):
    /// their albums, EPs and singles there.
    pub fn new_spotify(spotify_id: &str) -> Self {
        let page: Self = glib::Object::new();
        let imp = page.imp();
        imp.spotify_id.replace(Some(spotify_id.to_owned()));
        imp.actions.set_visible(false);
        imp.bio_box.set_visible(false);
        imp.identify_banner.set_revealed(false);
        imp.latin_name.set_visible(false);
        // No image choices: the picture is Spotify's.
        imp.portrait_button.set_menu_model(None::<&gio::MenuModel>);
        imp.portrait_button.set_can_target(false);
        page.refresh();
        page
    }

    pub fn artist_id(&self) -> i64 {
        self.imp().artist_id.get()
    }

    pub fn spotify_artist_id(&self) -> Option<String> {
        self.imp().spotify_id.borrow().clone()
    }

    /// The page's album panel collapses like the library's (window breakpoints).
    pub fn split_view(&self) -> adw::OverlaySplitView {
        self.imp().split_view.get()
    }

    fn refresh(&self) {
        if self.spotify_artist_id().is_some() {
            self.refresh_spotify();
            return;
        }
        let library = GatefoldApplication::get().library();
        let Some(artist) = library.artist(self.artist_id()) else {
            // The artist left the library; the window pops the page.
            return;
        };
        library.request_lookup(&artist);
        self.imp().artist.replace(Some(artist));
        self.refresh_header();
        self.fill_sections();
    }

    fn refresh_header(&self) {
        let imp = self.imp();
        let library = GatefoldApplication::get().library();
        let Some(artist) = library.artist(self.artist_id()) else {
            return;
        };
        let info = artist.artist();
        imp.name.set_label(&info.name);
        let latin = library.aliases().latin_name(&info.name);
        imp.latin_name.set_visible(latin.is_some());
        imp.latin_name
            .set_label(latin.as_deref().unwrap_or_default());
        let portrait = info
            .portrait_key
            .as_deref()
            .and_then(|key| covers::portrait(key, CoverSize::Panel));
        imp.portrait.set_texture(portrait.as_ref());
        imp.portrait.set_label(&info.name);
        // Commons images need their attribution (CONCEPT.md §8).
        let attribution = info
            .meta
            .custom_portrait_key
            .is_none()
            .then(|| info.meta.portrait_attribution.clone())
            .flatten();
        let tooltip = match attribution {
            Some(attribution) => format!("{}\n{attribution}", gettext("Change Image")),
            None => gettext("Change Image"),
        };
        imp.portrait_button.set_tooltip_text(Some(&tooltip));

        imp.counts.set_label(&format!(
            "{} · {}",
            ngettext("{} album", "{} albums", info.album_count)
                .replace("{}", &info.album_count.to_string()),
            format::song_count(info.track_count)
        ));

        let bio = info.meta.bio.clone().filter(|_| library.online_enabled());
        imp.bio_box.set_visible(bio.is_some());
        imp.bio.set_label(bio.as_deref().unwrap_or_default());

        self.fill_links(&artist);
        let state = info.meta.lookup_state.as_deref();
        let unclear = library.online_enabled() && matches!(state, Some("ambiguous" | "not-found"));
        imp.identify_banner.set_title(&match state {
            Some("ambiguous") => {
                // Translators: {} is an artist name.
                gettext("Several artists are called “{}”").replace("{}", &info.name)
            }
            _ => gettext("This artist wasn't found online"),
        });
        imp.identify_banner.set_revealed(unclear);
        imp.artist.replace(Some(artist));
    }

    fn fill_links(&self, artist: &ArtistObject) {
        let imp = self.imp();
        while let Some(child) = imp.links.first_child() {
            imp.links.remove(&child);
        }
        let info = artist.artist();
        let meta = &info.meta;
        let mut links: Vec<(String, String)> = Vec::new();
        if GatefoldApplication::get().library().online_enabled() {
            if let Some(id) = &meta.spotify_id {
                links.push((gettext("Spotify"), spotify_uri(id)));
            }
            if let Some(url) = &meta.wikipedia_url {
                links.push((gettext("Wikipedia"), url.clone()));
            }
            if let Some(mbid) = &info.mbid {
                links.push((
                    gettext("MusicBrainz"),
                    format!("https://musicbrainz.org/artist/{mbid}"),
                ));
            }
            if let Some(url) = &meta.website {
                links.push((gettext("Website"), url.clone()));
            }
        }
        imp.links.set_visible(!links.is_empty());
        for (label, uri) in links {
            let button = gtk::Button::builder()
                .label(&label)
                .tooltip_text(&uri)
                .css_classes(["pill", "small"])
                .build();
            button.connect_clicked(move |button| {
                let window = button.root().and_downcast::<gtk::Window>();
                gtk::UriLauncher::new(&uri).launch(
                    window.as_ref(),
                    gio::Cancellable::NONE,
                    |result| {
                        if let Err(err) = result {
                            tracing::warn!("Cannot open link: {err}");
                        }
                    },
                );
            });
            imp.links.append(&button);
        }
    }

    fn fill_sections(&self) {
        let imp = self.imp();
        while let Some(child) = imp.sections.first_child() {
            imp.sections.remove(&child);
        }
        imp.flows.borrow_mut().clear();
        let library = GatefoldApplication::get().library();
        let albums = library.artist_albums(self.artist_id());

        let pick = |filter: &dyn Fn(&Album, AlbumRole) -> bool| -> Vec<AlbumObject> {
            let mut groups: Vec<AlbumObject> = Vec::new();
            for (album, role) in &albums {
                if !filter(album, *role) {
                    continue;
                }
                if let Some(group) = library.album_group(album.id)
                    && !groups.iter().any(|g| g.group_key() == group.group_key())
                {
                    groups.push(group);
                }
            }
            groups
        };
        let own = |a: &Album, r: AlbumRole| r == AlbumRole::Credit && !a.soundtrack;
        let full = pick(&|a, r| own(a, r) && a.release_type == ReleaseType::Album);
        let short = pick(&|a, r| own(a, r) && a.release_type != ReleaseType::Album);
        let soundtracks = pick(&|a, r| r == AlbumRole::Credit && a.soundtrack);
        let collaborations = pick(&|_, r| r == AlbumRole::Collaboration);

        self.add_album_section(&gettext("Albums"), &full);
        self.add_album_section(&gettext("EPs & Singles"), &short);
        self.add_album_section(&gettext("Soundtracks"), &soundtracks);
        self.add_album_section(&gettext("Collaborations"), &collaborations);
        self.add_appears_on(&library.appears_on(self.artist_id()));

        let mut own_albums = full;
        own_albums.extend(short);
        own_albums.extend(soundtracks);
        imp.own_albums.replace(own_albums);
        self.fill_on_spotify();
    }

    /// The header and releases of a Spotify artist's page.
    fn refresh_spotify(&self) {
        let Some(id) = self.spotify_artist_id() else {
            return;
        };
        let generation = self.next_generation();
        glib::spawn_future_local(glib::clone!(
            #[weak(rename_to = page)]
            self,
            async move {
                let spotify = GatefoldApplication::get().spotify();
                if let Some(artist) = spotify.spotify_artist(&id).await {
                    page.show_spotify_header(&artist);
                }
                let albums = spotify.discography(&id).await;
                if page.imp().generation.get() != generation {
                    return;
                }
                page.clear_sections();
                let note = match &albums {
                    None => Some(gettext(
                        "Spotify can’t show this artist’s releases right now",
                    )),
                    Some(albums) if albums.is_empty() => {
                        Some(gettext("No albums, EPs or singles on Spotify"))
                    }
                    Some(_) => None,
                };
                if let Some(note) = note {
                    page.imp().sections.append(
                        &gtk::Label::builder()
                            .label(note)
                            .xalign(0.0)
                            .css_classes(["dim-label"])
                            .build(),
                    );
                }
                page.add_spotify_sections(albums.as_deref().unwrap_or_default(), false);
            }
        ));
    }

    fn show_spotify_header(&self, artist: &gatefold_core::db::SpotifyArtist) {
        let imp = self.imp();
        imp.name.set_label(&artist.name);
        imp.counts.set_label(&if artist.genres.is_empty() {
            gettext("On Spotify")
        } else {
            format!(
                "{} · {}",
                gettext("On Spotify"),
                artist
                    .genres
                    .iter()
                    .take(3)
                    .cloned()
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        });
        imp.portrait.set_label(&artist.name);
        imp.portrait.set_cover(artist.cover_key().as_deref());
        let id = artist.spotify_id.clone();
        while let Some(child) = imp.links.first_child() {
            imp.links.remove(&child);
        }
        // Like a library artist's links.
        let button = gtk::Button::builder()
            .label(gettext("Spotify"))
            .tooltip_text(gettext("Open in Spotify"))
            .css_classes(["pill", "small"])
            .build();
        button.connect_clicked(move |_| crate::spotify::open_on_spotify("artist", &id));
        imp.links.append(&button);
        imp.links.set_visible(true);
        if let Some(url) = artist.image_url.clone() {
            let portrait = imp.portrait.get();
            let key = artist.cover_key();
            crate::spotify::fetch_covers(vec![url], move || portrait.set_cover(key.as_deref()));
        }
    }

    fn next_generation(&self) -> u64 {
        let generation = self.imp().generation.get() + 1;
        self.imp().generation.set(generation);
        generation
    }

    fn clear_sections(&self) {
        let imp = self.imp();
        while let Some(child) = imp.sections.first_child() {
            imp.sections.remove(&child);
        }
        imp.flows.borrow_mut().clear();
    }

    /// Below a library artist's albums: their releases on Spotify that
    /// aren't in the library (§9.8).
    fn fill_on_spotify(&self) {
        let spotify = GatefoldApplication::get().spotify();
        let Some(artist) = self.imp().artist.borrow().clone() else {
            return;
        };
        let generation = self.next_generation();
        if !spotify.is_connected() {
            return;
        }
        let info = artist.artist();
        let name = info.name.clone();
        let known = info.meta.spotify_id.clone();
        glib::spawn_future_local(glib::clone!(
            #[weak(rename_to = page)]
            self,
            async move {
                let Some(id) = spotify.artist_id_for(&name, known.as_deref()).await else {
                    return;
                };
                let Some(albums) = spotify.discography(&id).await else {
                    return;
                };
                if page.imp().generation.get() != generation {
                    return;
                }
                let library = GatefoldApplication::get().library();
                let missing: Vec<_> = albums
                    .into_iter()
                    .filter(|album| library.local_album_for(album).is_none())
                    .collect();
                page.add_spotify_sections(&missing, true);
            }
        ));
    }

    /// Albums, and EPs and singles, from Spotify; with `heading`, under
    /// "On Spotify" (on a library artist's page).
    fn add_spotify_sections(&self, albums: &[gatefold_core::db::SpotifyAlbum], heading: bool) {
        if albums.is_empty() {
            return;
        }
        let imp = self.imp();
        if heading {
            imp.sections.append(
                &gtk::Label::builder()
                    .label(gettext("On Spotify"))
                    .xalign(0.0)
                    .margin_top(12)
                    .css_classes(["title-2"])
                    .build(),
            );
        }
        let (singles, full): (Vec<_>, Vec<_>) = albums.iter().cloned().partition(|a| a.is_single());
        self.add_spotify_section(&gettext("Albums"), full);
        self.add_spotify_section(&gettext("EPs & Singles"), singles);
    }

    fn add_spotify_section(&self, title: &str, albums: Vec<gatefold_core::db::SpotifyAlbum>) {
        if albums.is_empty() {
            return;
        }
        let section = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(12)
            .build();
        section.append(
            &gtk::Label::builder()
                .label(title)
                .xalign(0.0)
                .css_classes(["title-4"])
                .build(),
        );
        let columns = self.imp().columns.get().max(2);
        let flow = gtk::FlowBox::builder()
            .selection_mode(gtk::SelectionMode::None)
            .homogeneous(true)
            .min_children_per_line(columns)
            .max_children_per_line(columns)
            .column_spacing(18)
            .row_spacing(18)
            .build();
        let mut covers = Vec::new();
        let mut urls = Vec::new();
        for album in albums {
            let item = super::ShelfItem {
                cover_key: album.cover_key(),
                title: album.name.clone(),
                subtitle: album.year.map(|y| y.to_string()).unwrap_or_default(),
                artist: false,
            };
            let (tile, cover) = super::sized_tile(&item, 180);
            if let Some(url) = album.image_url.clone() {
                covers.push((url.clone(), cover, album.cover_key()));
                urls.push(url);
            }
            tile.connect_clicked(glib::clone!(
                #[weak(rename_to = page)]
                self,
                move |_| page.show_spotify_album(&album)
            ));
            flow.append(&tile);
        }
        crate::spotify::fetch_covers_in_batches(urls, move |ready| {
            for (url, cover, key) in &covers {
                if ready.contains(url) {
                    cover.set_cover(key.as_deref());
                }
            }
        });
        section.append(&flow);
        self.imp().sections.append(&section);
        self.imp().flows.borrow_mut().push(flow);
    }

    fn show_spotify_album(&self, album: &gatefold_core::db::SpotifyAlbum) {
        let imp = self.imp();
        imp.album_panel.set_spotify_album(album);
        imp.split_view.set_show_sidebar(true);
    }

    fn add_album_section(&self, title: &str, albums: &[AlbumObject]) {
        if albums.is_empty() {
            return;
        }
        let section = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(12)
            .build();
        section.append(
            &gtk::Label::builder()
                .label(title)
                .xalign(0.0)
                .css_classes(["title-4"])
                .build(),
        );
        // A fixed column count (see size_allocate), so a section with few
        // releases doesn't stretch its tiles: albums, EPs and singles all
        // look the same size.
        let columns = self.imp().columns.get().max(2);
        let flow = gtk::FlowBox::builder()
            .selection_mode(gtk::SelectionMode::None)
            .homogeneous(true)
            .min_children_per_line(columns)
            .max_children_per_line(columns)
            .column_spacing(18)
            .row_spacing(18)
            .activate_on_single_click(true)
            .build();
        for album in albums {
            let tile = AlbumTile::new();
            // Only cover and title: the page is the artist (§4.2).
            tile.bind_title_only(album);
            flow.append(&tile);
        }
        let albums = albums.to_vec();
        flow.connect_child_activated(glib::clone!(
            #[weak(rename_to = page)]
            self,
            move |_, child| {
                if let Some(album) = albums.get(child.index() as usize) {
                    page.show_album(album);
                }
            }
        ));
        section.append(&flow);
        self.imp().sections.append(&section);
        self.imp().flows.borrow_mut().push(flow);
    }

    fn apply_columns(&self) {
        let columns = self.imp().columns.get().max(2);
        for flow in self.imp().flows.borrow().iter() {
            flow.set_min_children_per_line(columns);
            flow.set_max_children_per_line(columns);
        }
    }

    fn add_appears_on(&self, tracks: &[TrackObject]) {
        if tracks.is_empty() {
            return;
        }
        let list = gtk::ListBox::builder()
            .selection_mode(gtk::SelectionMode::None)
            .css_classes(["boxed-list"])
            .build();
        for track in tracks {
            let row = adw::ActionRow::builder()
                .use_markup(false)
                .title(track.title())
                .subtitle(
                    [
                        track.track().album_title.clone(),
                        track.track().album_artist.clone(),
                    ]
                    .into_iter()
                    .flatten()
                    .collect::<Vec<_>>()
                    .join(" — "),
                )
                .activatable(true)
                .build();
            let cover: CoverImage = glib::Object::builder()
                .property("min-size", 40)
                .property("natural-size", 40)
                .build();
            cover.set_cover(track.cover_key());
            cover.set_valign(gtk::Align::Center);
            row.add_prefix(&cover);
            row.add_suffix(&appears_on_actions(track));
            row.add_suffix(
                &gtk::Label::builder()
                    .label(format::duration(track.duration_ms()))
                    .css_classes(["dim-label", "numeric"])
                    .build(),
            );
            list.append(&row);
        }
        let count = tracks.len();
        let tracks = tracks.to_vec();
        list.connect_row_activated(glib::clone!(
            #[weak(rename_to = page)]
            self,
            move |_, row| {
                let Some(track) = tracks.get(row.index() as usize) else {
                    return;
                };
                let library = GatefoldApplication::get().library();
                if let Some(album) = track
                    .track()
                    .album_id
                    .and_then(|id| library.album_group(id))
                {
                    page.show_album(&album);
                    page.imp().album_panel.highlight_track(track.id());
                }
            }
        ));
        // Always shown, after the artist's own releases (§4.2).
        let section = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(12)
            .build();
        section.append(
            &gtk::Label::builder()
                .label(
                    ngettext("Appears On ({})", "Appears On ({})", count as u32)
                        .replace("{}", &count.to_string()),
                )
                .xalign(0.0)
                .css_classes(["title-4"])
                .build(),
        );
        section.append(&list);
        self.imp().sections.append(&section);
    }

    fn show_album(&self, album: &AlbumObject) {
        let imp = self.imp();
        imp.album_panel.set_album(album);
        imp.split_view.set_show_sidebar(true);
    }

    fn play_all(&self, shuffle: bool) {
        let library = GatefoldApplication::get().library();
        let tracks: Vec<TrackObject> = self
            .imp()
            .own_albums
            .borrow()
            .iter()
            .flat_map(|album| library.album_tracks(album.album().id))
            .collect();
        let name = self
            .imp()
            .artist
            .borrow()
            .as_ref()
            .map(|a| a.artist().name.clone());
        GatefoldApplication::get()
            .player()
            .play_tracks(tracks, None, shuffle, name);
    }

    async fn choose_image(&self) {
        let filter = gtk::FileFilter::new();
        filter.set_name(Some(&gettext("Images")));
        filter.add_mime_type("image/*");
        let filters = gio::ListStore::new::<gtk::FileFilter>();
        filters.append(&filter);
        let dialog = gtk::FileDialog::builder()
            .title(gettext("Choose Artist Image"))
            .modal(true)
            .filters(&filters)
            .build();
        let window = self.root().and_downcast::<gtk::Window>();
        let file = match dialog.open_future(window.as_ref()).await {
            Ok(file) => file,
            Err(err) => {
                if !err.matches(gtk::DialogError::Dismissed) {
                    tracing::warn!("File chooser failed: {err}");
                }
                return;
            }
        };
        let library = GatefoldApplication::get().library();
        let result = match file.load_bytes_future().await {
            Ok((bytes, _)) => {
                library
                    .set_custom_portrait(self.artist_id(), bytes.to_vec())
                    .await
            }
            Err(err) => Err(err.to_string()),
        };
        if let Err(err) = result {
            tracing::warn!("Cannot use the image: {err}");
            library.emit_by_name::<()>("message", &[&gettext("This image can't be used")]);
        }
    }
}

/// The artist on Spotify (§4.2): the web page, or with the setting (and
/// the app installed) a `spotify:` link for the app, which the Linux
/// Spotify app takes as "play".
fn spotify_uri(id: &str) -> String {
    let in_app = GatefoldApplication::get()
        .player()
        .settings()
        .boolean("spotify-links-in-app");
    if in_app && gio::AppInfo::default_for_uri_scheme("spotify").is_some() {
        format!("spotify:artist:{id}")
    } else {
        format!("https://open.spotify.com/artist/{id}")
    }
}

/// Play (its album, from this song), Add to Queue and Add to Playlist for
/// an "Appears On" song.
fn appears_on_actions(track: &TrackObject) -> gtk::Box {
    let button = |icon: &str, tooltip: String| {
        gtk::Button::builder()
            .icon_name(icon)
            .tooltip_text(tooltip)
            .valign(gtk::Align::Center)
            .css_classes(["flat", "circular"])
            .build()
    };
    let play = button("media-playback-start-symbolic", gettext("Play"));
    play.connect_clicked(glib::clone!(
        #[strong]
        track,
        move |_| {
            let Some(album_id) = track.track().album_id else {
                return;
            };
            let library = GatefoldApplication::get().library();
            let index = library
                .album_tracks(album_id)
                .iter()
                .position(|t| t.id() == track.id());
            GatefoldApplication::get()
                .player()
                .play_album(album_id, index, false);
        }
    ));
    let queue = button("list-add-symbolic", gettext("Add to Queue"));
    queue.connect_clicked(glib::clone!(
        #[strong]
        track,
        move |_| {
            GatefoldApplication::get()
                .player()
                .add_to_queue(vec![track.clone()]);
        }
    ));
    let playlist = button("view-list-bullet-symbolic", gettext("Add to Playlist…"));
    let track_id = track.id();
    playlist.connect_clicked(move |button| {
        super::playlist_picker::show(button, vec![track_id]);
    });
    let actions = gtk::Box::builder().spacing(3).build();
    actions.append(&play);
    actions.append(&queue);
    actions.append(&playlist);
    actions
}
