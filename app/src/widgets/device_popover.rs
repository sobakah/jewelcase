//! "Play On…" (CONCEPT.md §9.3): this computer or a Spotify Connect
//! device, and the banner while a device plays.

use adw::prelude::*;
use gatefold_spotify::Device;
use gettextrs::gettext;
use gtk::glib;

use crate::application::GatefoldApplication;

pub fn setup(button: &gtk::MenuButton, popover: &gtk::Popover, banner: &adw::Banner) {
    let app = GatefoldApplication::get();
    let spotify = app.spotify();
    let player = app.player();

    // Only with a Spotify account.
    let show = glib::clone!(
        #[weak]
        button,
        move |spotify: &crate::spotify::Spotify| button.set_visible(spotify.is_connected())
    );
    show(&spotify);
    spotify.connect_status_notify(move |spotify| show(spotify));

    let update_banner = glib::clone!(
        #[weak]
        banner,
        #[weak]
        button,
        move |player: &crate::player::Player| {
            match player.remote_device() {
                // Soloist plays here, for the Spotify app or a followed
                // playlist: say so, and how to get back.
                Some(_) if player.remote_receiver() => {
                    banner.set_title(&gettext(
                        "Spotify plays this through Soloist; the controls steer Spotify",
                    ));
                    banner.set_button_label(Some(&gettext("_Back to My Queue")));
                    banner.set_revealed(true);
                    button.remove_css_class("accent");
                }
                Some(device) => {
                    // Translators: {} is a device name, e.g. "Kitchen".
                    banner.set_title(&gettext("Playing on {}").replace("{}", &device));
                    banner.set_button_label(Some(&gettext("Play _Here")));
                    banner.set_revealed(true);
                    button.add_css_class("accent");
                }
                None => {
                    banner.set_revealed(false);
                    button.remove_css_class("accent");
                }
            }
        }
    );
    update_banner(&player);
    let on_receiver = update_banner.clone();
    player.connect_remote_receiver_notify(move |player| on_receiver(player));
    player.connect_remote_device_notify(move |player| update_banner(player));
    banner.connect_button_clicked(|_| continue_here());

    popover.connect_show(fill);
}

/// Back to this computer; a library song continues here where it was.
fn continue_here() {
    let app = GatefoldApplication::get();
    let player = app.player();
    // From Soloist's playback back to Gatefold's own queue, paused.
    if player.remote_receiver() {
        app.spotify().play_here(true);
        return;
    }
    let track = player.current_track().filter(|t| t.id() > 0);
    let (position, was_playing) = (player.position(), player.playing());
    app.spotify().play_here(true);
    if let Some(track) = track
        && was_playing
    {
        player.play_now(track);
        player.seek(position);
    }
}

fn fill(popover: &gtk::Popover) {
    let content = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(6)
        .width_request(260)
        .margin_top(6)
        .margin_bottom(6)
        .margin_start(6)
        .margin_end(6)
        .build();
    content.append(
        &gtk::Label::builder()
            .label(gettext("Play On"))
            .xalign(0.0)
            .margin_start(6)
            .css_classes(["heading"])
            .build(),
    );
    let list = gtk::ListBox::builder()
        .selection_mode(gtk::SelectionMode::None)
        .css_classes(["boxed-list"])
        .build();
    content.append(&list);
    let player = GatefoldApplication::get().player();
    let here = device_row(
        "computer-symbolic",
        &gettext("This Computer"),
        !player.is_remote(),
        true,
    );
    here.connect_activated(glib::clone!(
        #[weak]
        popover,
        move |_| {
            popover.popdown();
            continue_here();
        }
    ));
    list.append(&here);
    let loading = adw::Spinner::builder()
        .margin_top(6)
        .margin_bottom(6)
        .build();
    content.append(&loading);
    popover.set_child(Some(&content));

    glib::spawn_future_local(glib::clone!(
        #[weak]
        popover,
        #[weak]
        list,
        #[weak]
        loading,
        #[weak]
        content,
        async move {
            let spotify = GatefoldApplication::get().spotify();
            let result = spotify.devices().await;
            loading.set_visible(false);
            let devices = match result {
                Ok(devices) => devices,
                Err(err) => {
                    tracing::warn!("Cannot list Spotify devices: {err}");
                    content.append(&note(&gettext("Spotify can't be reached")));
                    return;
                }
            };
            if devices.is_empty() {
                content.append(&note(&gettext(
                    "No Spotify devices. Open Spotify on your phone, speaker or computer to see it here.",
                )));
            }
            let current = spotify.remote_device_id();
            // Soloist streams for Gatefold: it's "This Computer" already.
            let own = spotify.soloist_device_name();
            for device in devices
                .into_iter()
                .filter(|d| own.as_deref() != Some(d.name.as_str()))
            {
                let usable = device.id.is_some() && !device.is_restricted;
                let row = device_row(
                    device.icon_name(),
                    &device.name,
                    current.is_some() && device.id == current,
                    usable,
                );
                row.connect_activated(glib::clone!(
                    #[weak]
                    popover,
                    #[strong]
                    device,
                    move |_| {
                        popover.popdown();
                        play_on(device.clone());
                    }
                ));
                list.append(&row);
            }
        }
    ));
}

fn play_on(device: Device) {
    glib::spawn_future_local(async move {
        GatefoldApplication::get().spotify().play_on(device).await;
    });
}

fn device_row(icon: &str, name: &str, current: bool, usable: bool) -> adw::ActionRow {
    let row = adw::ActionRow::builder()
        .use_markup(false)
        .title(name)
        .activatable(usable && !current)
        .sensitive(usable)
        .build();
    row.add_prefix(&gtk::Image::from_icon_name(icon));
    if current {
        row.add_suffix(&gtk::Image::from_icon_name("object-select-symbolic"));
    }
    row
}

fn note(text: &str) -> gtk::Label {
    gtk::Label::builder()
        .label(text)
        .wrap(true)
        .max_width_chars(30)
        .xalign(0.0)
        .margin_start(6)
        .css_classes(["dim-label", "caption"])
        .build()
}
