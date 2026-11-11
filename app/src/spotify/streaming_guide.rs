//! Preferences › Spotify › Streaming: the steps to set up Spotify Soloist,
//! like the steps to a client ID (CONCEPT.md §9.4). Two ways: with soloistd
//! (it installs Soloist, keeps it up to date and runs it in the background)
//! or without (Jewelcase starts a Soloist the user downloaded). Each step
//! shows a check mark once it's done, looked at again whenever the window
//! comes back (the user ran a command in a terminal) and when streaming's
//! status changes.

use std::cell::Cell;
use std::rc::Rc;

use adw::prelude::*;
use gettextrs::gettext;
use gtk::{gio, glib};
use jewelcase_soloist::soloistd::{self, ServiceState};

use super::Spotify;

/// A numbered step; the number turns into a check mark once it's done.
#[derive(Clone)]
struct Step {
    row: adw::ActionRow,
    number: gtk::Label,
    check: gtk::Image,
}

impl Step {
    fn new(number: u32, title: &str, subtitle: Option<&str>) -> Self {
        let row = adw::ActionRow::builder()
            .use_markup(false)
            .subtitle_selectable(subtitle.is_some())
            .build();
        row.set_title(title);
        row.set_subtitle(subtitle.unwrap_or_default());
        let number = gtk::Label::builder()
            .label(number.to_string())
            .width_chars(2)
            .css_classes(["dim-label", "numeric"])
            .build();
        let check = gtk::Image::builder()
            .icon_name("object-select-symbolic")
            .visible(false)
            .css_classes(["success"])
            .build();
        crate::widgets::set_accessible_label(&check, &gettext("Done"));
        row.add_prefix(&number);
        row.add_prefix(&check);
        Self { row, number, check }
    }

    /// A button that opens `url`.
    fn link(self, url: &'static str) -> Self {
        let button = gtk::Button::builder()
            .icon_name("adw-external-link-symbolic")
            .tooltip_text(gettext("Open in the Browser"))
            .valign(gtk::Align::Center)
            .css_classes(["flat"])
            .build();
        button.connect_clicked(move |_| super::open_in_browser(url));
        self.row.add_suffix(&button);
        self
    }

    /// A button that copies `text` (a command).
    fn copy(self, text: &'static str) -> Self {
        let button = gtk::Button::builder()
            .icon_name("edit-copy-symbolic")
            .valign(gtk::Align::Center)
            .css_classes(["flat"])
            .build();
        crate::widgets::set_accessible_label(&button, &gettext("Copy"));
        button.connect_clicked(move |button| {
            button.clipboard().set_text(text);
            button.set_icon_name("object-select-symbolic");
        });
        self.row.add_suffix(&button);
        self
    }

    fn set_done(&self, done: bool) {
        self.number.set_visible(!done);
        self.check.set_visible(done);
    }
}

/// What's set up, read on a worker thread.
struct Found {
    soloistd: bool,
    config: Option<soloistd::Config>,
    /// soloistd downloaded Soloist.
    soloistd_soloist: bool,
    service: ServiceState,
    /// soloistd's Soloist answers on its WebSocket address.
    listening: bool,
    /// A Soloist Jewelcase can start itself.
    own_soloist: bool,
}

/// The two ways, as expandable rows for the Streaming group.
pub struct Guide {
    pub with_soloistd: adw::ExpanderRow,
    pub without_soloistd: adw::ExpanderRow,
    /// Looks at what's set up again.
    pub refresh: Rc<dyn Fn()>,
}

pub fn guide(spotify: &Spotify) -> Guide {
    let flatpak = crate::config::is_flatpak();

    // With soloistd. A Flatpak uses the soloistd on the computer, whose
    // commands it can't run and whose settings it can only read.
    let with_soloistd = adw::ExpanderRow::builder()
        .title(gettext("Set Up With soloistd"))
        .build();
    let releases: &'static str = "https://github.com/MopigamesYT/soloistd/releases";
    let a: Vec<Step> = if flatpak {
        vec![
            Step::new(
                1,
                &gettext("Get your Soloist API key"),
                Some(&gettext(
                    "On Spotify’s developer dashboard. There’s one per Spotify account: if you have one, use it",
                )),
            )
            .link(jewelcase_soloist::API_KEY_URL),
            Step::new(
                2,
                &gettext("On your computer, install soloistd and run this in a terminal"),
                Some("soloistd setup"),
            )
            .copy("soloistd setup")
            .link(releases),
            Step::new(
                3,
                &gettext("Let Jewelcase control it"),
                Some(&gettext(
                    "Turn on “Control by Jewelcase” in the installed Jewelcase’s Preferences, or add \"websocket\": \"127.0.0.1:9090\" to ~/.config/soloistd/config.json",
                )),
            ),
            Step::new(
                4,
                &gettext("Then run it in the background, also after you log in"),
                Some("soloistd service install"),
            )
            .copy("soloistd service install"),
            Step::new(5, &gettext("Turn on “Stream From Spotify” below"), None),
            Step::new(6, &gettext("Link Soloist once"), None),
        ]
    } else {
        vec![
        Step::new(
            1,
            &gettext("Get your Soloist API key"),
            Some(&gettext(
                "On Spotify’s developer dashboard. There’s one per Spotify account: if you have one, use it",
            )),
        )
        .link(jewelcase_soloist::API_KEY_URL),
        Step::new(
            2,
            &gettext("Install soloistd"),
            Some(&gettext("From its releases page: the RPM for Fedora")),
        )
        .link(releases),
        Step::new(
            3,
            &gettext("In a terminal, run this; it asks for the API key and downloads Soloist"),
            Some("soloistd setup"),
        )
        .copy("soloistd setup"),
        Step::new(
            4,
            &gettext("Then this, so it runs in the background, also after you log in"),
            Some("soloistd service install"),
        )
        .copy("soloistd service install"),
        Step::new(
            5,
            &gettext("Turn on “Control by Jewelcase” under soloistd below"),
            None,
        ),
        Step::new(6, &gettext("Turn on “Stream From Spotify” below"), None),
        Step::new(7, &gettext("Link Soloist once"), None),
        ]
    };
    for step in &a {
        with_soloistd.add_row(&step.row);
    }

    // Without soloistd: Jewelcase starts Soloist itself (in a Flatpak the
    // one soloistd downloaded, without soloistd's service).
    let without_soloistd = adw::ExpanderRow::builder()
        .title(if flatpak {
            gettext("Set Up Without soloistd’s Service")
        } else {
            gettext("Set Up Without soloistd")
        })
        .build();
    let b = [
        Step::new(
            1,
            &gettext("Enter your Soloist API key under “Soloist API Key” below"),
            Some(&gettext(
                "From Spotify’s developer dashboard. There’s one per Spotify account: the one you use elsewhere (with soloistd, say) works here too",
            )),
        )
        .link(jewelcase_soloist::API_KEY_URL),
        if flatpak {
            // The Flatpak runs the Soloist soloistd downloads on the computer.
            Step::new(
                2,
                &gettext("Install soloistd on your computer and run “soloistd setup” once"),
                Some(&gettext(
                    "Jewelcase runs the Soloist it downloads and keeps up to date",
                )),
            )
            .link(releases)
        } else {
            Step::new(
                2,
                &gettext("Download Soloist for Linux from Spotify"),
                Some(&gettext(
                    "Unpack it and put the “soloist” program into ~/.local/bin. Builds expire after 90 days; then download the new one.",
                )),
            )
            .link(jewelcase_soloist::DOWNLOADS_URL)
        },
        Step::new(
            3,
            &gettext("Turn on “Stream From Spotify” below"),
            Some(&gettext("Jewelcase starts Soloist while it runs")),
        ),
        Step::new(4, &gettext("Link Soloist once"), None),
    ];
    for step in &b {
        without_soloistd.add_row(&step.row);
    }

    let generation = Rc::new(Cell::new(0u64));
    // The way being set up opens by itself, once.
    let opened = Rc::new(Cell::new(false));
    let refresh: Rc<dyn Fn()> = Rc::new(glib::clone!(
        #[weak]
        spotify,
        #[weak]
        with_soloistd,
        #[weak]
        without_soloistd,
        move || {
            let current = generation.get() + 1;
            generation.set(current);
            let generation = generation.clone();
            let opened = opened.clone();
            let (a, b) = (a.clone(), b.clone());
            glib::spawn_future_local(async move {
                let configured = spotify.settings().string("soloist-path");
                let Ok(found) = gio::spawn_blocking(move || {
                    let configured = (!configured.trim().is_empty())
                        .then(|| std::path::PathBuf::from(configured.trim()));
                    let config = soloistd::Config::load();
                    // Its address as Jewelcase reads it (loopback only).
                    let listening = std::fs::read_to_string(soloistd::config_path())
                        .ok()
                        .and_then(|text| jewelcase_soloist::soloistd_websocket(&text))
                        .and_then(|(addr, port)| {
                            let addr: std::net::IpAddr = addr.parse().ok()?;
                            std::net::TcpStream::connect_timeout(
                                &(addr, port).into(),
                                std::time::Duration::from_millis(300),
                            )
                            .ok()
                        })
                        .is_some();
                    Found {
                        soloistd: soloistd::binary().is_some(),
                        config,
                        listening,
                        soloistd_soloist: soloistd::soloist_version().is_some(),
                        service: soloistd::service_state(),
                        own_soloist: jewelcase_soloist::find_binary(configured.as_deref())
                            .is_some(),
                    }
                })
                .await
                else {
                    return;
                };
                let own_key = matches!(
                    super::keyring::load_soloist_key().await,
                    Ok(Some(key)) if !key.trim().is_empty()
                );
                if generation.get() != current {
                    return;
                }
                let status = spotify.stream_status();
                let ready = status == "ready";
                let streaming = spotify.settings().boolean("spotify-streaming");
                let config = found.config.as_ref();

                // With soloistd.
                let device = config
                    .and_then(soloistd::Config::device_name)
                    .unwrap_or_else(|| "Soloist".to_owned());
                // Translators: {} is a device name.
                a[a.len() - 1].row.set_subtitle(
                    &gettext(
                        "Choose “{}” in the Spotify app’s device list on your phone or computer",
                    )
                    .replace("{}", &device),
                );
                let key = config.is_some_and(soloistd::Config::has_api_key);
                let set_up = config.is_some() && found.soloistd_soloist;
                let websocket = config.is_some_and(|c| c.websocket().is_some());
                let done_a = if flatpak {
                    vec![key, set_up, websocket, found.listening, streaming, ready]
                } else {
                    vec![
                        key,
                        found.soloistd,
                        set_up,
                        !matches!(
                            found.service,
                            ServiceState::NotInstalled | ServiceState::Unknown
                        ),
                        websocket,
                        streaming,
                        ready,
                    ]
                };
                // Without.
                b[3].row.set_subtitle(
                    &gettext(
                        "Choose “{}” in the Spotify app’s device list on your phone or computer",
                    )
                    .replace("{}", super::soloist::own_device_name()),
                );
                let done_b = [
                    own_key,
                    found.own_soloist,
                    streaming && !matches!(status.as_str(), "off" | "missing" | "needs-key"),
                    ready,
                ];
                for (step, done) in a.iter().zip(done_a.iter().copied()) {
                    step.set_done(done);
                }
                for (step, done) in b.iter().zip(done_b) {
                    step.set_done(done);
                }
                let summary = |done: &[bool], what: String| {
                    if done.iter().all(|d| *d) {
                        gettext("Set up")
                    } else {
                        what
                    }
                };
                with_soloistd.set_subtitle(&summary(
                    &done_a,
                    gettext("Recommended: keeps Soloist up to date and running in the background"),
                ));
                without_soloistd.set_subtitle(&summary(
                    &done_b,
                    if flatpak {
                        gettext("Jewelcase starts Soloist while it runs")
                    } else {
                        gettext("Jewelcase starts Soloist while it runs; you download new builds yourself")
                    },
                ));
                // Not working yet: the way that's begun (soloistd, unless
                // only the other one is) opens, once.
                if !ready && !opened.replace(true) {
                    let begun_b = done_b[..2].iter().any(|d| *d);
                    let begun_a = done_a[..4].iter().any(|d| *d);
                    if with_soloistd.is_visible() && (begun_a || !begun_b) {
                        with_soloistd.set_expanded(true);
                    } else {
                        without_soloistd.set_expanded(true);
                    }
                }
            });
        }
    ));

    // Shown (and back from a terminal or the browser): look again.
    let watching = Rc::new(Cell::new(false));
    for row in [&with_soloistd, &without_soloistd] {
        let refresh = refresh.clone();
        let watching = watching.clone();
        row.connect_map(move |row| {
            refresh();
            if watching.replace(true) {
                return;
            }
            let Some(window) = row.root().and_downcast::<gtk::Window>() else {
                return;
            };
            let refresh = refresh.clone();
            let id = window.connect_is_active_notify(move |window| {
                if window.is_active() {
                    refresh();
                }
            });
            crate::widgets::disconnect_with(row, &window, id);
        });
    }
    Guide {
        with_soloistd,
        without_soloistd,
        refresh,
    }
}
