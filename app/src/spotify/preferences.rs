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

fn step(number: u32, title: &str, subtitle: Option<&str>) -> adw::ActionRow {
    let row = adw::ActionRow::builder()
        .use_markup(false)
        .title(title)
        .subtitle(subtitle.unwrap_or_default())
        .subtitle_selectable(subtitle.is_some())
        .build();
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
