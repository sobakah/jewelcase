//! The popovers next to Shuffle and Repeat (CONCEPT.md §7): shuffling
//! songs or albums, mixing in similar songs and keeping the music going;
//! and the repeat modes.

use adw::prelude::*;
use gatefold_core::queue::RepeatMode;
use gettextrs::gettext;
use gtk::{gio, glib};

use crate::application::GatefoldApplication;

/// Shuffle's options; all settings, so they apply right away.
pub fn shuffle_popover() -> gtk::Popover {
    let player = GatefoldApplication::get().player();
    let settings = player.settings().clone();

    let content = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(12)
        .margin_top(12)
        .margin_bottom(12)
        .margin_start(12)
        .margin_end(12)
        .width_request(300)
        .build();

    // Songs or whole albums.
    let songs = gtk::ToggleButton::builder()
        .label(gettext("Songs"))
        .hexpand(true)
        .build();
    let albums = gtk::ToggleButton::builder()
        .label(gettext("Albums"))
        .hexpand(true)
        .group(&songs)
        .build();
    settings.bind("shuffle-albums", &albums, "active").build();
    songs.set_active(!albums.is_active());
    albums
        .bind_property("active", &songs, "active")
        .invert_boolean()
        .bidirectional()
        .build();
    let mode = gtk::Box::builder()
        .css_classes(["linked"])
        .homogeneous(true)
        .build();
    mode.append(&songs);
    mode.append(&albums);
    content.append(&heading(&gettext("Shuffle")));
    content.append(&mode);

    content.append(&gtk::Separator::new(gtk::Orientation::Horizontal));

    // Mix in similar songs.
    let (mix_row, mix_switch) = switch_row(&gettext("Mix In Similar Songs"));
    settings.bind("mix-similar", &mix_switch, "active").build();
    content.append(&mix_row);

    let options = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(6)
        .margin_start(6)
        .build();
    mix_switch
        .bind_property("active", &options, "sensitive")
        .sync_create()
        .build();
    let genre = check(&gettext("Same Genre"));
    settings.bind("mix-genre", &genre, "active").build();
    let tempo = check("");
    settings.bind("mix-bpm", &tempo, "active").build();
    let tempo_label = glib::clone!(
        #[weak]
        tempo,
        move |settings: &gio::Settings| {
            // Translators: {} is a percentage, e.g. "Similar Tempo (±8 %)".
            tempo.set_label(Some(
                &gettext("Similar Tempo (±{} %)")
                    .replace("{}", &settings.int("mix-bpm-tolerance").to_string()),
            ));
        }
    );
    tempo_label(&settings);
    settings.connect_changed(Some("mix-bpm-tolerance"), move |settings, _| {
        tempo_label(settings)
    });
    let artist = check(&gettext("Same Artist"));
    settings.bind("mix-artist", &artist, "active").build();
    let sound = check(&gettext("Sounds Similar"));
    settings.bind("mix-sound", &sound, "active").build();
    for widget in [&genre, &tempo, &artist, &sound] {
        options.append(widget);
    }

    // Tempo and sound need the analysis; say how far it is.
    let analysis_note = gtk::Label::builder()
        .xalign(0.0)
        .wrap(true)
        .css_classes(["dim-label", "caption"])
        .build();
    options.append(&analysis_note);
    let start_analysis = gtk::Button::builder()
        .label(gettext("Analyze Songs in the Background"))
        .halign(gtk::Align::Start)
        .css_classes(["pill", "small"])
        .build();
    start_analysis.connect_clicked(glib::clone!(
        #[strong]
        settings,
        move |_| {
            let _ = settings.set_string("analysis-mode", "background");
            let _ = settings.set_boolean("analysis-asked", true);
        }
    ));
    options.append(&start_analysis);
    let analyzer = GatefoldApplication::get().analyzer();
    let update_note = glib::clone!(
        #[weak]
        analysis_note,
        #[weak]
        start_analysis,
        #[strong]
        settings,
        move |analyzer: &crate::analyzer::Analyzer| {
            let (done, total) = (analyzer.analyzed(), analyzer.total());
            let off = settings.string("analysis-mode") == "off";
            analysis_note.set_visible(done < total);
            start_analysis.set_visible(off && done < total);
            let text = if off {
                gettext("Tempo and sound need the song analysis, which is off.")
            } else {
                // Translators: smart shuffle; {done} and {total} are song counts.
                gettext("Tempo and sound: {done} of {total} songs analyzed so far")
                    .replace("{done}", &done.to_string())
                    .replace("{total}", &total.to_string())
            };
            analysis_note.set_label(&text);
        }
    );
    update_note(&analyzer);
    let on_progress = update_note.clone();
    analyzer.connect_analyzed_notify(move |analyzer| on_progress(analyzer));
    let on_total = update_note.clone();
    analyzer.connect_total_notify(move |analyzer| on_total(analyzer));
    let on_mode = analyzer.clone();
    settings.connect_changed(Some("analysis-mode"), move |_, _| update_note(&on_mode));

    // How often.
    let every = gtk::SpinButton::with_range(1.0, 5.0, 1.0);
    every.set_valign(gtk::Align::Center);
    settings.bind("mix-every", &every, "value").build();
    let rate = gtk::Box::builder().spacing(6).margin_top(6).build();
    rate.append(
        &gtk::Label::builder()
            // Translators: smart shuffle, "One after every [3] songs".
            .label(gettext("One after every"))
            .build(),
    );
    rate.append(&every);
    rate.append(&gtk::Label::new(Some(&gettext("songs"))));
    options.append(&rate);
    content.append(&options);

    content.append(&gtk::Separator::new(gtk::Orientation::Horizontal));

    let (keep_row, keep_switch) =
        switch_row(&gettext("Keep Playing Similar Music When the Queue Ends"));
    settings
        .bind("keep-playing-similar", &keep_switch, "active")
        .build();
    content.append(&keep_row);

    gtk::Popover::builder().child(&content).build()
}

/// Repeat Off / All / One, as radio buttons.
pub fn repeat_popover() -> gtk::Popover {
    let player = GatefoldApplication::get().player();
    let content = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(6)
        .margin_top(12)
        .margin_bottom(12)
        .margin_start(12)
        .margin_end(12)
        .build();
    content.append(&heading(&gettext("Repeat")));
    let modes = [
        (RepeatMode::Off, gettext("Off")),
        (RepeatMode::All, gettext("Repeat All")),
        (RepeatMode::One, gettext("Repeat One Song")),
    ];
    let mut group: Option<gtk::CheckButton> = None;
    let mut buttons = Vec::new();
    for (mode, label) in modes {
        let button = gtk::CheckButton::with_label(&label);
        if let Some(first) = &group {
            button.set_group(Some(first));
        } else {
            group = Some(button.clone());
        }
        button.connect_toggled(glib::clone!(
            #[weak]
            player,
            move |button| {
                if button.is_active() && player.repeat() != mode.as_str() {
                    player.set_repeat(mode);
                }
            }
        ));
        content.append(&button);
        buttons.push((mode, button));
    }
    let sync = move |player: &crate::player::Player| {
        for (mode, button) in &buttons {
            if player.repeat() == mode.as_str() && !button.is_active() {
                button.set_active(true);
            }
        }
    };
    sync(&player);
    player.connect_repeat_notify(move |player| sync(player));
    gtk::Popover::builder().child(&content).build()
}

fn heading(text: &str) -> gtk::Label {
    gtk::Label::builder()
        .label(text)
        .xalign(0.0)
        .css_classes(["heading"])
        .build()
}

fn check(label: &str) -> gtk::CheckButton {
    gtk::CheckButton::with_label(label)
}

fn switch_row(label: &str) -> (gtk::Box, gtk::Switch) {
    let switch = gtk::Switch::builder().valign(gtk::Align::Center).build();
    let row = gtk::Box::builder().spacing(12).build();
    row.append(
        &gtk::Label::builder()
            .label(label)
            .xalign(0.0)
            .wrap(true)
            .hexpand(true)
            .build(),
    );
    row.append(&switch);
    (row, switch)
}
