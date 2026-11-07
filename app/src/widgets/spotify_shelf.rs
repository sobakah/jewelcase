//! A shelf of Spotify albums or artists above a library grid (CONCEPT.md
//! §9.8): apart from the library, collapsed by default like followed
//! playlists, one row that scrolls sideways (the grid below stays a fast,
//! virtual list).

use std::cell::{OnceCell, RefCell};
use std::rc::Rc;

use adw::prelude::*;
use gettextrs::gettext;
use gtk::glib;
use gtk::subclass::prelude::*;

use super::CoverImage;
use crate::application::GatefoldApplication;
use crate::config::APP_ID;

/// One tile.
pub struct ShelfItem {
    pub cover_key: Option<String>,
    pub title: String,
    pub subtitle: String,
    /// Artists show a person when there's no picture.
    pub artist: bool,
}

type Activate = Rc<dyn Fn(usize)>;

mod imp {
    use super::*;

    #[derive(Default)]
    pub struct SpotifyShelf {
        pub toggle: gtk::Button,
        pub content: adw::ButtonContent,
        pub revealer: gtk::Revealer,
        pub row: gtk::Box,
        pub permission: gtk::Box,
        pub settings: OnceCell<gio::Settings>,
        /// The setting that keeps the shelf open.
        pub key: OnceCell<String>,
        /// "Saved on Spotify ({})".
        pub title: OnceCell<String>,
        pub count: std::cell::Cell<usize>,
        pub needs_permission: std::cell::Cell<bool>,
        pub activate: RefCell<Option<Activate>>,
    }

    use gtk::gio;

    #[glib::object_subclass]
    impl ObjectSubclass for SpotifyShelf {
        const NAME: &'static str = "GatefoldSpotifyShelf";
        type Type = super::SpotifyShelf;
        type ParentType = gtk::Box;
    }

    impl ObjectImpl for SpotifyShelf {
        fn constructed(&self) {
            self.parent_constructed();
            let obj = self.obj();
            obj.set_orientation(gtk::Orientation::Vertical);
            obj.add_css_class("spotify-shelf");
            obj.set_visible(false);

            self.content.set_icon_name("pan-end-symbolic");
            self.toggle.set_child(Some(&self.content));
            self.toggle.set_halign(gtk::Align::Start);
            self.toggle.add_css_class("flat");
            self.toggle.add_css_class("heading");
            obj.append(&self.toggle);

            let inner = gtk::Box::new(gtk::Orientation::Vertical, 6);
            // Older logins lack the permission to read the library.
            self.permission.set_spacing(12);
            self.permission.set_margin_start(12);
            self.permission.set_margin_end(12);
            self.permission.set_margin_bottom(12);
            let label = gtk::Label::builder()
                .label(gettext(
                    "Connect to Spotify again to allow Gatefold to read your saved albums and followed artists.",
                ))
                .wrap(true)
                .xalign(0.0)
                .hexpand(true)
                .css_classes(["dim-label"])
                .build();
            let button = gtk::Button::builder()
                .label(gettext("_Connect Again"))
                .use_underline(true)
                .valign(gtk::Align::Center)
                .css_classes(["pill"])
                .build();
            button.connect_clicked(|_| {
                let spotify = GatefoldApplication::get().spotify();
                glib::spawn_future_local(async move { spotify.connect().await });
            });
            self.permission.append(&label);
            self.permission.append(&button);
            self.permission.set_visible(false);
            inner.append(&self.permission);

            self.row.set_spacing(18);
            self.row.set_halign(gtk::Align::Start);
            self.row.add_css_class("shelf-row");
            let scroller = gtk::ScrolledWindow::builder()
                .hscrollbar_policy(gtk::PolicyType::Automatic)
                .vscrollbar_policy(gtk::PolicyType::Never)
                .child(&self.row)
                .build();
            inner.append(&scroller);
            self.revealer
                .set_transition_type(gtk::RevealerTransitionType::SlideDown);
            self.revealer.set_child(Some(&inner));
            obj.append(&self.revealer);

            self.toggle.connect_clicked(glib::clone!(
                #[weak]
                obj,
                move |_| {
                    let imp = obj.imp();
                    if let (Some(settings), Some(key)) = (imp.settings.get(), imp.key.get()) {
                        let open = settings.boolean(key);
                        let _ = settings.set_boolean(key, !open);
                    }
                }
            ));
        }
    }

    impl WidgetImpl for SpotifyShelf {}
    impl BoxImpl for SpotifyShelf {}
}

glib::wrapper! {
    pub struct SpotifyShelf(ObjectSubclass<imp::SpotifyShelf>)
        @extends gtk::Widget, gtk::Box,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget, gtk::Orientable;
}

impl SpotifyShelf {
    /// `title` has a `{}` for the count; `key` is the boolean setting that
    /// keeps the shelf open.
    pub fn setup(&self, title: &str, key: &str) {
        let imp = self.imp();
        let settings = gtk::gio::Settings::new(APP_ID);
        settings.connect_changed(
            Some(key),
            glib::clone!(
                #[weak(rename_to = shelf)]
                self,
                move |_, _| shelf.update()
            ),
        );
        let _ = imp.settings.set(settings);
        let _ = imp.key.set(key.to_owned());
        let _ = imp.title.set(title.to_owned());
        self.update();
    }

    pub fn set_items(&self, items: Vec<ShelfItem>, activate: impl Fn(usize) + 'static) {
        let imp = self.imp();
        while let Some(child) = imp.row.first_child() {
            imp.row.remove(&child);
        }
        let activate: Activate = Rc::new(activate);
        for (index, item) in items.iter().enumerate() {
            let tile = tile(item);
            let activate = activate.clone();
            tile.connect_clicked(move |_| activate(index));
            imp.row.append(&tile);
        }
        imp.activate.replace(Some(activate));
        imp.count.set(items.len());
        self.update();
    }

    pub fn set_needs_permission(&self, needs: bool) {
        self.imp().needs_permission.set(needs);
        self.update();
    }

    fn update(&self) {
        let imp = self.imp();
        let count = imp.count.get();
        let needs = imp.needs_permission.get();
        let open = match (imp.settings.get(), imp.key.get()) {
            (Some(settings), Some(key)) => settings.boolean(key),
            _ => false,
        };
        self.set_visible(count > 0 || needs);
        if let Some(title) = imp.title.get() {
            imp.content
                .set_label(&title.replace("{}", &count.to_string()));
        }
        imp.content.set_icon_name(if open {
            "pan-down-symbolic"
        } else {
            "pan-end-symbolic"
        });
        imp.permission.set_visible(needs);
        imp.revealer.set_reveal_child(open);
    }
}

impl Default for SpotifyShelf {
    fn default() -> Self {
        glib::Object::new()
    }
}

fn tile(item: &ShelfItem) -> gtk::Button {
    sized_tile(item, 128).0
}

/// Cover, title and a dim line, as the grid's tiles; also the cover, to
/// show it once downloaded.
pub fn sized_tile(item: &ShelfItem, natural: i32) -> (gtk::Button, CoverImage) {
    // On the shelf a fixed size (it scrolls sideways; a stretched row
    // mustn't make covers huge); in a grid, as wide as the column.
    let cover: CoverImage = glib::Object::builder()
        .property("min-size", if natural <= 128 { natural } else { 120 })
        .property("natural-size", natural)
        .build();
    if item.artist {
        cover.set_placeholder_icon("avatar-default-symbolic");
    }
    cover.set_cover(item.cover_key.as_deref());
    cover.set_label(&item.title);
    cover.set_margin_bottom(6);
    let label = |text: &str, classes: &[&str]| {
        gtk::Label::builder()
            .label(text)
            .xalign(0.0)
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .width_chars(1)
            .max_width_chars(1)
            .css_classes(classes)
            .build()
    };
    let content = gtk::Box::new(gtk::Orientation::Vertical, 3);
    content.set_width_request(natural.min(128));
    content.append(&cover);
    content.append(&label(&item.title, &["album-title"]));
    if !item.subtitle.is_empty() {
        content.append(&label(&item.subtitle, &["dim-label"]));
    }
    let button = gtk::Button::builder()
        .child(&content)
        .tooltip_text(&item.title)
        .css_classes(["flat", "shelf-tile"])
        .build();
    (button, cover)
}
