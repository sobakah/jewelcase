//! An artist's page (CONCEPT.md §4.2): portrait, counts, Play all, links,
//! a short bio, and the albums grouped by release type, plus
//! Collaborations and a collapsed "Appears on".

use std::cell::{Cell, RefCell};

use adw::prelude::*;
use adw::subclass::prelude::*;
use gatefold_core::db::Album;
use gatefold_core::library::artists::AlbumRole;
use gatefold_core::library::covers::CoverSize;
use gatefold_core::library::titles::ReleaseType;
use gettextrs::{gettext, ngettext};
use gtk::{gio, glib};

use super::{AlbumPanel, AlbumTile};
use crate::application::GatefoldApplication;
use crate::objects::{AlbumObject, ArtistObject, TrackObject};
use crate::{covers, format};

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
        pub avatar: TemplateChild<adw::Avatar>,
        #[template_child]
        pub name: TemplateChild<gtk::Label>,
        #[template_child]
        pub counts: TemplateChild<gtk::Label>,
        #[template_child]
        pub links: TemplateChild<gtk::Box>,
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

        pub artist_id: Cell<i64>,
        pub artist: RefCell<Option<ArtistObject>>,
        /// The artist's own albums in page order, for Play All.
        pub own_albums: RefCell<Vec<AlbumObject>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for ArtistPage {
        const NAME: &'static str = "GatefoldArtistPage";
        type Type = super::ArtistPage;
        type ParentType = adw::NavigationPage;

        fn class_init(klass: &mut Self::Class) {
            AlbumPanel::ensure_type();
            klass.bind_template();
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
        fn constructed(&self) {
            self.parent_constructed();
            let obj = self.obj();
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

    impl WidgetImpl for ArtistPage {}
    impl NavigationPageImpl for ArtistPage {}
}

glib::wrapper! {
    pub struct ArtistPage(ObjectSubclass<imp::ArtistPage>)
        @extends gtk::Widget, adw::NavigationPage,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl ArtistPage {
    pub fn new(artist_id: i64) -> Self {
        let page: Self = glib::Object::new();
        page.imp().artist_id.set(artist_id);
        page.refresh();
        page
    }

    pub fn artist_id(&self) -> i64 {
        self.imp().artist_id.get()
    }

    /// The page's album panel collapses like the library's (window breakpoints).
    pub fn split_view(&self) -> adw::OverlaySplitView {
        self.imp().split_view.get()
    }

    fn refresh(&self) {
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
        self.set_title(&info.name);
        imp.name.set_label(&info.name);
        imp.avatar.set_text(Some(&info.name));
        let portrait = info
            .portrait_key
            .as_deref()
            .and_then(|key| covers::portrait(key, CoverSize::Panel));
        imp.avatar.set_custom_image(portrait.as_ref());
        // Commons images need their attribution (CONCEPT.md §8).
        let attribution = info
            .meta
            .custom_portrait_key
            .is_none()
            .then(|| info.meta.portrait_attribution.clone())
            .flatten();
        imp.avatar.set_tooltip_text(attribution.as_deref());

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
        let flow = gtk::FlowBox::builder()
            .selection_mode(gtk::SelectionMode::None)
            .homogeneous(true)
            .min_children_per_line(2)
            .max_children_per_line(8)
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
    }

    fn add_appears_on(&self, tracks: &[TrackObject]) {
        if tracks.is_empty() {
            return;
        }
        let list = gtk::ListBox::builder()
            .selection_mode(gtk::SelectionMode::None)
            .css_classes(["boxed-list"])
            .margin_top(12)
            .build();
        for track in tracks {
            let row = adw::ActionRow::builder()
                .title(track.title())
                .subtitle(track.track().album_title.clone().unwrap_or_default())
                .use_markup(false)
                .activatable(true)
                .build();
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
                }
            }
        ));
        // Collapsed by default (§4.2).
        let expander = gtk::Expander::builder()
            .label(
                ngettext("Appears On ({})", "Appears On ({})", count as u32)
                    .replace("{}", &count.to_string()),
            )
            .child(&list)
            .build();
        expander.add_css_class("appears-on");
        self.imp().sections.append(&expander);
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
        GatefoldApplication::get()
            .player()
            .play_tracks(tracks, shuffle);
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

/// `spotify:` if the Spotify app handles it, else the web player (§4.2).
fn spotify_uri(id: &str) -> String {
    if gio::AppInfo::default_for_uri_scheme("spotify").is_some() {
        format!("spotify:artist:{id}")
    } else {
        format!("https://open.spotify.com/artist/{id}")
    }
}
