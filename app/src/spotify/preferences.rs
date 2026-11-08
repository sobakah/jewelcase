//! The Spotify page in Preferences (CONCEPT.md §9.1, §9.3): the client ID
//! with a guided setup, connecting, and the playlist options.

use adw::prelude::*;
use gatefold_spotify::auth::REDIRECT_URI;
use gettextrs::gettext;
use gtk::glib;

use super::Spotify;
use crate::application::GatefoldApplication;

const DASHBOARD: &str = "https://developer.spotify.com/dashboard";

pub fn page() -> adw::PreferencesPage {
    let spotify = GatefoldApplication::get().spotify();
    let settings = spotify.settings().clone();
    let page = adw::PreferencesPage::builder()
        .name("spotify")
        .title(gettext("Spotify"))
        .icon_name("folder-remote-symbolic")
        .build();

    // The account.
    let account = adw::PreferencesGroup::builder()
        .title(gettext("Spotify Account"))
        .description(gettext(
            "Control your Spotify devices from Gatefold and use your Spotify playlists here; songs you have play from your library. Needs Spotify Premium and a free developer app of your own.",
        ))
        .build();
    let status = adw::ActionRow::builder().use_markup(false).build();
    let button = gtk::Button::builder()
        .valign(gtk::Align::Center)
        .css_classes(["pill"])
        .build();
    status.add_suffix(&button);
    let spinner = adw::Spinner::builder().valign(gtk::Align::Center).build();
    status.add_suffix(&spinner);
    account.add(&status);

    let client_id = adw::EntryRow::builder().title(gettext("Client ID")).build();
    settings
        .bind("spotify-client-id", &client_id, "text")
        .build();
    account.add(&client_id);

    // The guided setup (§9.1).
    let guide = adw::ExpanderRow::builder()
        .title(gettext("How to Get a Client ID"))
        .subtitle(gettext("Takes about two minutes"))
        .build();
    let open = gtk::Button::builder()
        .icon_name("adw-external-link-symbolic")
        .tooltip_text(gettext("Open in the Browser"))
        .valign(gtk::Align::Center)
        .css_classes(["flat"])
        .build();
    open.connect_clicked(|_| super::open_in_browser(DASHBOARD));
    let step1 = step(
        1,
        &gettext("Open the Spotify developer dashboard and log in"),
        Some(DASHBOARD),
    );
    step1.add_suffix(&open);
    guide.add_row(&step1);
    guide.add_row(&step(
        2,
        &gettext("Create an app; any name and description will do"),
        None,
    ));
    let copy = gtk::Button::builder()
        .icon_name("edit-copy-symbolic")
        .tooltip_text(gettext("Copy"))
        .valign(gtk::Align::Center)
        .css_classes(["flat"])
        .build();
    copy.connect_clicked(|button| {
        button.clipboard().set_text(REDIRECT_URI);
        button.set_icon_name("object-select-symbolic");
    });
    let step3 = step(
        3,
        &gettext("Add this redirect URI exactly"),
        Some(REDIRECT_URI),
    );
    step3.add_suffix(&copy);
    guide.add_row(&step3);
    guide.add_row(&step(
        4,
        &gettext("Choose “Web API”, save, and copy the client ID into the field above"),
        None,
    ));
    guide.add_row(&step(
        5,
        &gettext("Connect here and allow access in the browser"),
        None,
    ));
    account.add(&guide);
    page.add(&account);

    // Playlists.
    let playlists = adw::PreferencesGroup::builder()
        .title(gettext("Playlists"))
        .build();
    let show = adw::SwitchRow::builder()
        .title(gettext("Show _Spotify Playlists"))
        .subtitle(gettext(
            "Your Spotify playlists next to the local ones. Playlists created on Spotify from Gatefold stay linked either way.",
        ))
        .use_underline(true)
        .build();
    settings.bind("spotify-playlists", &show, "active").build();
    show.connect_active_notify(glib::clone!(
        #[weak]
        spotify,
        move |_| spotify.sync_soon()
    ));
    playlists.add(&show);
    let sync = adw::ButtonRow::builder()
        .title(gettext("Sync _Now"))
        .use_underline(true)
        .start_icon_name("view-refresh-symbolic")
        .build();
    sync.connect_activated(glib::clone!(
        #[weak]
        spotify,
        move |_| spotify.sync_soon()
    ));
    playlists.add(&sync);
    page.add(&playlists);
    page.add(&streaming_group(&spotify));
    // soloistd: only with Soloist as the way to stream.
    let soloistd = super::soloistd_prefs::group(&spotify);
    let show_soloistd = glib::clone!(
        #[weak]
        soloistd,
        move |spotify: &Spotify| {
            soloistd.set_visible(spotify.stream_backend() == super::stream::Backend::Soloist);
        }
    );
    show_soloistd(&spotify);
    settings.connect_changed(
        Some("spotify-stream-backend"),
        glib::clone!(
            #[weak]
            spotify,
            move |_, _| show_soloistd(&spotify)
        ),
    );
    page.add(&soloistd);

    let update = glib::clone!(
        #[weak]
        status,
        #[weak]
        button,
        #[weak]
        spinner,
        #[weak]
        client_id,
        #[weak]
        sync,
        #[weak]
        playlists,
        move |spotify: &Spotify| {
            let connected = spotify.status() == "connected";
            let connecting = spotify.status() == "connecting";
            status.set_title(&match (connected, connecting) {
                // Translators: {} is a Spotify account name.
                (true, _) => gettext("Connected as {}")
                    .replace("{}", &spotify.account_name().unwrap_or_default()),
                (_, true) => gettext("Waiting for the Browser…"),
                _ => gettext("Not Connected"),
            });
            status.set_subtitle(&if connecting {
                gettext("Allow Gatefold access on the Spotify page that opened")
            } else {
                String::new()
            });
            button.set_label(&if connected {
                gettext("Disconnect")
            } else if connecting {
                gettext("Cancel")
            } else {
                gettext("Connect")
            });
            if connected || connecting {
                button.remove_css_class("suggested-action");
            } else {
                button.add_css_class("suggested-action");
            }
            spinner.set_visible(connecting || spotify.syncing());
            // The client ID belongs to the login.
            client_id.set_sensitive(!connected && !connecting);
            playlists.set_sensitive(connected);
            sync.set_sensitive(!spotify.syncing());
        }
    );
    update(&spotify);
    for property in ["status", "account-name", "syncing"] {
        let update = update.clone();
        spotify.connect_notify_local(Some(property), move |spotify, _| update(spotify));
    }
    button.connect_clicked(glib::clone!(
        #[weak]
        spotify,
        #[weak]
        page,
        move |_| match spotify.status().as_str() {
            "connected" => {
                glib::spawn_future_local(glib::clone!(
                    #[weak]
                    spotify,
                    #[weak]
                    page,
                    async move {
                        if confirm_disconnect(&page).await {
                            spotify.disconnect().await;
                        }
                    }
                ));
            }
            "connecting" => spotify.cancel_connect(),
            _ => {
                glib::spawn_future_local(async move { spotify.connect().await });
            }
        }
    ));
    page
}

/// Streaming (§9.4): Spotify Soloist (official) or librespot (unofficial,
/// only in builds with it), with the switch and what each needs.
fn streaming_group(spotify: &Spotify) -> adw::PreferencesGroup {
    use super::stream::Backend;
    let settings = spotify.settings().clone();
    let group = adw::PreferencesGroup::builder()
        .title(gettext("Streaming"))
        .description(gettext(
            "Plays Spotify songs that aren’t in your library. Your own files always play from your library.",
        ))
        .build();

    // Which way (only offered when librespot is built in).
    let backend = adw::ComboRow::builder()
        .title(gettext("Streams _With"))
        .use_underline(true)
        .model(&gtk::StringList::new(&[
            &gettext("Spotify Soloist (official)"),
            &gettext("librespot (unofficial)"),
        ]))
        .visible(Spotify::streaming_built())
        .build();
    backend.set_selected(u32::from(spotify.stream_backend() == Backend::Librespot));
    backend.connect_selected_notify(glib::clone!(
        #[weak]
        spotify,
        move |row| {
            let value = if row.selected() == 1 {
                "librespot"
            } else {
                "soloist"
            };
            if spotify.settings().string("spotify-stream-backend") == value {
                return;
            }
            let _ = spotify
                .settings()
                .set_string("spotify-stream-backend", value);
            glib::spawn_future_local(async move { spotify.switch_stream_backend().await });
        }
    ));
    group.add(&backend);

    let about = adw::ActionRow::builder().use_markup(false).build();
    about.add_css_class("property");
    group.add(&about);

    let switch = adw::SwitchRow::builder()
        .title(gettext("Stream _From Spotify"))
        .use_underline(true)
        .build();
    settings
        .bind("spotify-streaming", &switch, "active")
        .build();
    switch.connect_active_notify(glib::clone!(
        #[weak]
        spotify,
        move |switch| {
            let on = switch.is_active();
            let running = !matches!(
                spotify.stream_status().as_str(),
                "off" | "missing" | "needs-key" | "error" | "expired"
            );
            glib::spawn_future_local(async move {
                if on && !running {
                    spotify.enable_streaming().await;
                } else if !on {
                    spotify.disable_streaming(true).await;
                }
            });
        }
    ));
    group.add(&switch);

    // Soloist: the program (or soloistd) and the API key.
    let program = {
        let row = adw::ActionRow::builder().use_markup(false).build();
        row.set_title(&gettext("Soloist"));
        row
    };
    let download = gtk::Button::builder()
        .label(gettext("Download Page"))
        .valign(gtk::Align::Center)
        .css_classes(["flat"])
        .build();
    download.connect_clicked(|_| super::open_in_browser(gatefold_soloist::DOWNLOADS_URL));
    program.add_suffix(&download);
    group.add(&program);
    let key = adw::PasswordEntryRow::builder()
        .title(gettext("Soloist API Key"))
        .show_apply_button(true)
        .build();
    let get_key = gtk::Button::builder()
        .icon_name("adw-external-link-symbolic")
        .tooltip_text(gettext("Get an API Key"))
        .valign(gtk::Align::Center)
        .css_classes(["flat"])
        .build();
    get_key.connect_clicked(|_| super::open_in_browser(gatefold_soloist::API_KEY_URL));
    key.add_suffix(&get_key);
    key.connect_apply(glib::clone!(
        #[weak]
        spotify,
        move |row| {
            let text = row.text().trim().to_owned();
            row.set_text("");
            glib::spawn_future_local(async move {
                let saved = if text.is_empty() {
                    super::keyring::delete_soloist_key().await
                } else {
                    super::keyring::save_soloist_key(text).await
                };
                match saved {
                    Ok(()) => {
                        spotify.message(&gettext("Soloist API key saved"));
                        if spotify.settings().boolean("spotify-streaming") {
                            spotify.start_streaming();
                        }
                    }
                    Err(err) => {
                        tracing::warn!("Cannot store the Soloist API key: {err}");
                        spotify.message(&gettext("The API key couldn't be saved in the keyring"));
                    }
                }
            });
        }
    ));
    group.add(&key);
    // Spotify's songs are louder than ReplayGain'd files.
    let offset = adw::SpinRow::builder()
        .title(gettext("Volume of Spotify Songs"))
        .subtitle(gettext("In dB; 0 matches your files with ReplayGain"))
        .adjustment(&gtk::Adjustment::new(0.0, -15.0, 6.0, 1.0, 3.0, 0.0))
        .build();
    settings
        .bind("spotify-volume-offset", &offset, "value")
        .build();
    offset.connect_value_notify(glib::clone!(
        #[weak]
        spotify,
        move |_| spotify.stream_volume(GatefoldApplication::get().player().volume())
    ));
    group.add(&offset);

    let status = {
        let row = adw::ActionRow::builder().use_markup(false).build();
        row.set_title(&gettext("Status"));
        row
    };
    group.add(&status);

    let update = glib::clone!(
        #[weak]
        about,
        #[weak]
        switch,
        #[weak]
        program,
        #[weak]
        download,
        #[weak]
        key,
        #[weak]
        offset,
        #[weak]
        status,
        move |spotify: &Spotify| {
            let soloist = spotify.stream_backend() == Backend::Soloist;
            about.set_title(&if soloist {
                gettext(
                    "Spotify Soloist is Spotify’s own headless player; it plays through your speakers, Gatefold controls it. Get it from Spotify (soloistd can install and update it), plus a free Soloist API key. Needs Spotify Premium. Spotify’s terms don’t clearly cover using it from another app.",
                )
            } else {
                gettext(
                    "librespot is an unofficial Spotify client; songs play through Gatefold’s own output. Needs Spotify Premium. Using it goes against Spotify’s terms of use, and Spotify can stop it from working at any time (on some accounts it already doesn’t).",
                )
            });
            switch.set_subtitle(&if soloist {
                String::new()
            } else {
                gettext("Turning it on opens a second Spotify login in the browser, as librespot")
            });
            program.set_visible(soloist);
            key.set_visible(soloist);
            offset.set_visible(soloist);
            if soloist {
                let running = gatefold_soloist::find_running();
                let configured = spotify.settings().string("soloist-path");
                let configured = (!configured.trim().is_empty())
                    .then(|| std::path::PathBuf::from(configured.trim()));
                let binary = gatefold_soloist::find_binary(configured.as_deref());
                let (text, missing) = match (&running, &binary) {
                    (Some(running), _) => (
                        // Translators: {} is "soloistd" or "soloist".
                        gettext("Running ({}); Gatefold uses it, no API key needed here")
                            .replace("{}", running.source),
                        false,
                    ),
                    (None, Some(binary)) => (
                        gatefold_soloist::version(binary)
                            .unwrap_or_else(|| binary.display().to_string()),
                        false,
                    ),
                    (None, None) if gatefold_soloist::soloistd_installed() => (
                        gettext(
                            "soloistd is set up but its WebSocket API is off: add \"websocket\": \"127.0.0.1:9090\" to its config.json and restart it",
                        ),
                        false,
                    ),
                    (None, None) => (gettext("Not installed"), true),
                };
                program.set_subtitle(&text);
                download.set_visible(missing);
                // Only needed when Gatefold starts Soloist itself.
                key.set_sensitive(running.is_none());
            }
            status.set_subtitle(&match spotify.stream_status().as_str() {
                "ready" => gettext("Ready: songs that aren’t in your library play from Spotify"),
                "connecting" => gettext("Connecting…"),
                "pairing" => gettext(
                    "Link Soloist once: in the Spotify app on your phone or computer, open the device list and choose it",
                ),
                "missing" => gettext("Soloist isn’t installed (see above)"),
                "needs-key" => gettext("Enter your Soloist API key above"),
                "expired" => gettext(
                    "This Soloist build has expired: download the new one (soloistd updates it by itself)",
                ),
                "error" => gettext("Not working right now"),
                _ => gettext("Off"),
            });
        }
    );
    update(spotify);
    spotify.connect_stream_status_notify(glib::clone!(
        #[strong]
        update,
        move |spotify| update(spotify)
    ));
    settings.connect_changed(
        Some("spotify-stream-backend"),
        glib::clone!(
            #[weak]
            spotify,
            move |_, _| update(&spotify)
        ),
    );
    group
}

fn step(number: u32, title: &str, subtitle: Option<&str>) -> adw::ActionRow {
    let row = {
        let row = adw::ActionRow::builder()
            .use_markup(false)
            .subtitle_selectable(subtitle.is_some())
            .build();
        row.set_title(title);
        row.set_subtitle(subtitle.unwrap_or_default());
        row
    };
    row.add_prefix(
        &gtk::Label::builder()
            .label(number.to_string())
            .width_chars(2)
            .css_classes(["dim-label", "numeric"])
            .build(),
    );
    row
}

async fn confirm_disconnect(parent: &impl IsA<gtk::Widget>) -> bool {
    let dialog = adw::AlertDialog::builder()
        .heading(gettext("Disconnect from Spotify?"))
        .body(gettext(
            "Spotify playlists are removed from Gatefold; they stay on Spotify. Playlists created on Spotify from Gatefold stay here as local playlists.",
        ))
        .close_response("cancel")
        .default_response("cancel")
        .build();
    dialog.add_responses(&[
        ("cancel", &gettext("_Cancel")),
        ("disconnect", &gettext("_Disconnect")),
    ]);
    dialog.set_response_appearance("disconnect", adw::ResponseAppearance::Destructive);
    dialog.choose_future(Some(parent)).await == "disconnect"
}
