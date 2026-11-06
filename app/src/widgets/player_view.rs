//! The full player in the expanded bottom sheet (CONCEPT.md §4.3), with a
//! basic queue pane (§6).

use adw::subclass::prelude::*;
use gettextrs::gettext;
use gtk::glib;
use gtk::prelude::*;

use super::CoverImage;
use super::mini_player::update_play_button;
use crate::application::GatefoldApplication;
use crate::format;
use crate::objects::TrackObject;
use crate::player::Player;

mod imp {
    use super::*;

    #[derive(Debug, Default, gtk::CompositeTemplate)]
    #[template(resource = "/io/github/sobakah/Gatefold/ui/player_view.ui")]
    pub struct PlayerView {
        #[template_child]
        pub pane_stack: TemplateChild<gtk::Stack>,
        #[template_child]
        pub cover: TemplateChild<CoverImage>,
        #[template_child]
        pub queue_list: TemplateChild<gtk::ListBox>,
        #[template_child]
        pub title: TemplateChild<gtk::Label>,
        #[template_child]
        pub subtitle: TemplateChild<gtk::Label>,
        #[template_child]
        pub position_label: TemplateChild<gtk::Label>,
        #[template_child]
        pub seek: TemplateChild<gtk::Scale>,
        #[template_child]
        pub seek_adjustment: TemplateChild<gtk::Adjustment>,
        #[template_child]
        pub duration_label: TemplateChild<gtk::Label>,
        #[template_child]
        pub play_button: TemplateChild<gtk::Button>,
        #[template_child]
        pub repeat_button: TemplateChild<gtk::Button>,
        #[template_child]
        pub queue_button: TemplateChild<gtk::ToggleButton>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for PlayerView {
        const NAME: &'static str = "GatefoldPlayerView";
        type Type = super::PlayerView;
        type ParentType = adw::Bin;

        fn class_init(klass: &mut Self::Class) {
            CoverImage::ensure_type();
            klass.bind_template();
        }

        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for PlayerView {
        fn constructed(&self) {
            self.parent_constructed();
            self.obj().setup();
        }
    }

    impl WidgetImpl for PlayerView {}
    impl BinImpl for PlayerView {}
}

glib::wrapper! {
    pub struct PlayerView(ObjectSubclass<imp::PlayerView>)
        @extends gtk::Widget, adw::Bin,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl PlayerView {
    fn setup(&self) {
        let imp = self.imp();
        let player = GatefoldApplication::get().player();

        player.connect_current_track_notify(glib::clone!(
            #[weak(rename_to = view)]
            self,
            move |player| view.update_track(player)
        ));
        player.connect_playing_notify(glib::clone!(
            #[weak(rename_to = view)]
            self,
            move |player| update_play_button(&view.imp().play_button, player.playing())
        ));
        player.connect_position_notify(glib::clone!(
            #[weak(rename_to = view)]
            self,
            move |player| view.update_position(player)
        ));
        player.connect_duration_notify(glib::clone!(
            #[weak(rename_to = view)]
            self,
            move |player| view.update_position(player)
        ));
        player.connect_repeat_notify(glib::clone!(
            #[weak(rename_to = view)]
            self,
            move |player| view.update_repeat(player)
        ));

        imp.seek.connect_change_value(glib::clone!(
            #[weak]
            player,
            #[upgrade_or]
            glib::Propagation::Proceed,
            move |_, _, value| {
                player.seek(value.max(0.0) as u64);
                glib::Propagation::Proceed
            }
        ));

        imp.queue_button.connect_toggled(glib::clone!(
            #[weak(rename_to = view)]
            self,
            move |button| {
                let page = if button.is_active() { "queue" } else { "cover" };
                view.imp().pane_stack.set_visible_child_name(page);
            }
        ));

        imp.queue_list.bind_model(
            Some(player.queue_model()),
            glib::clone!(
                #[weak]
                player,
                #[upgrade_or_panic]
                move |item| {
                    let track = item
                        .downcast_ref::<TrackObject>()
                        .expect("queue holds tracks");
                    queue_row(&player, track)
                }
            ),
        );
        imp.queue_list.set_header_func(|row, _| {
            let label = match row.index() {
                0 => gettext("Now Playing"),
                1 => gettext("Next"),
                _ => {
                    row.set_header(None::<&gtk::Widget>);
                    return;
                }
            };
            row.set_header(Some(
                &gtk::Label::builder()
                    .label(label)
                    .xalign(0.0)
                    .margin_start(12)
                    .margin_top(12)
                    .margin_bottom(6)
                    .css_classes(["dim-label", "caption-heading"])
                    .build(),
            ));
        });
        imp.queue_list.connect_row_activated(glib::clone!(
            #[weak]
            player,
            move |_, row| {
                if row.index() > 0 {
                    player.jump_to_upcoming(row.index() as usize);
                }
            }
        ));

        self.update_track(&player);
        self.update_repeat(&player);
        update_play_button(&imp.play_button, player.playing());
    }

    fn update_track(&self, player: &Player) {
        let imp = self.imp();
        let track = player.current_track();
        imp.cover
            .set_cover(track.as_ref().and_then(|t| t.cover_key()));
        match &track {
            Some(track) => {
                imp.title.set_label(&track.title());
                let artist = track.artist();
                let album = track.track().album_title.clone();
                let subtitle = match (artist, album) {
                    (Some(artist), Some(album)) => format!("{artist} — {album}"),
                    (Some(text), None) | (None, Some(text)) => text,
                    (None, None) => String::new(),
                };
                imp.subtitle.set_label(&subtitle);
                // Translators: accessible label of the cover; {} is a song title.
                imp.cover
                    .set_label(&gettext("Cover of {}").replace("{}", &track.title()));
            }
            None => {
                imp.title.set_label(&gettext("Not Playing"));
                imp.subtitle.set_label(&gettext("Pick an album to start"));
            }
        }
        imp.seek.set_sensitive(track.is_some());
        self.update_position(player);
    }

    fn update_position(&self, player: &Player) {
        let imp = self.imp();
        let duration = player.duration();
        imp.seek_adjustment.set_upper(duration.max(1) as f64);
        imp.seek_adjustment.set_value(player.position() as f64);
        imp.position_label
            .set_label(&format::duration(player.position()));
        imp.duration_label.set_label(&format::duration(duration));
    }

    fn update_repeat(&self, player: &Player) {
        let (icon, tooltip) = match player.repeat().as_str() {
            "all" => ("media-playlist-repeat-symbolic", gettext("Repeat: All")),
            "one" => (
                "media-playlist-repeat-song-symbolic",
                gettext("Repeat: One Song"),
            ),
            _ => (
                "media-playlist-consecutive-symbolic",
                gettext("Repeat: Off"),
            ),
        };
        let button = &self.imp().repeat_button;
        button.set_icon_name(icon);
        button.set_tooltip_text(Some(&tooltip));
    }
}

fn queue_row(player: &Player, track: &TrackObject) -> gtk::Widget {
    let cover: CoverImage = glib::Object::builder()
        .property("min-size", 40)
        .property("natural-size", 40)
        .build();
    cover.set_cover(track.cover_key());
    let text = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .valign(gtk::Align::Center)
        .hexpand(true)
        .build();
    text.append(
        &gtk::Label::builder()
            .label(track.title())
            .xalign(0.0)
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .build(),
    );
    if let Some(artist) = track.artist() {
        text.append(
            &gtk::Label::builder()
                .label(artist)
                .xalign(0.0)
                .ellipsize(gtk::pango::EllipsizeMode::End)
                .css_classes(["dim-label", "caption"])
                .build(),
        );
    }
    let line = gtk::Box::builder()
        .spacing(12)
        .margin_start(12)
        .margin_end(6)
        .margin_top(6)
        .margin_bottom(6)
        .build();
    line.append(&cover);
    line.append(&text);
    line.append(
        &gtk::Label::builder()
            .label(format::duration(track.duration_ms()))
            .css_classes(["dim-label", "numeric"])
            .build(),
    );

    let row = gtk::ListBoxRow::builder().child(&line).build();
    let remove = gtk::Button::builder()
        .icon_name("list-remove-symbolic")
        .tooltip_text(gettext("Remove from Queue"))
        .valign(gtk::Align::Center)
        .css_classes(["flat", "circular"])
        .build();
    remove.connect_clicked(glib::clone!(
        #[weak]
        player,
        #[weak]
        row,
        move |_| {
            if row.index() > 0 {
                player.remove_upcoming(row.index() as usize);
            }
        }
    ));
    line.append(&remove);
    // The current track can't be removed; its row only shows.
    row.connect_map(glib::clone!(
        #[weak]
        remove,
        move |row| remove.set_visible(row.index() > 0)
    ));
    row.upcast()
}
