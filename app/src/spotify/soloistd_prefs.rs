//! soloistd in Preferences (CONCEPT.md §9.4): its service, its settings
//! (written to its config.json, then the service restarts), updating
//! Soloist, linking again and the log.

use std::cell::Cell;
use std::rc::Rc;

use adw::prelude::*;
use gatefold_soloist::soloistd::{self, Config, ServiceState};
use gettextrs::gettext;
use gtk::{gio, glib};

use super::Spotify;

/// A change to soloistd's settings, made on a worker thread.
type ConfigChange = Box<dyn FnOnce(&mut Config) + Send>;

/// What the rows show, read on a worker thread.
struct Snapshot {
    installed: bool,
    config: Option<Config>,
    service: ServiceState,
    soloist: Option<String>,
}

fn snapshot() -> Snapshot {
    Snapshot {
        installed: soloistd::binary().is_some(),
        config: Config::load(),
        service: soloistd::service_state(),
        soloist: soloistd::soloist_version(),
    }
}

pub fn group(spotify: &Spotify) -> adw::PreferencesGroup {
    let group = adw::PreferencesGroup::builder()
        .title(gettext("soloistd"))
        .description(gettext(
            "soloistd installs Spotify Soloist, keeps it up to date and runs it in the background; Gatefold then uses it.",
        ))
        .build();
    let project = gtk::Button::builder()
        .icon_name("adw-external-link-symbolic")
        .tooltip_text(gettext("Project Page"))
        .valign(gtk::Align::Center)
        .css_classes(["flat"])
        .build();
    project.connect_clicked(|_| super::open_in_browser(soloistd::PROJECT_URL));
    group.set_header_suffix(Some(&project));

    let missing = adw::ActionRow::builder()
        .title(gettext("Not Installed"))
        .subtitle(gettext(
            "Install soloistd and run “soloistd setup” and “soloistd service install”, then open Preferences again",
        ))
        .build();
    group.add(&missing);

    // The service.
    let service = adw::ActionRow::builder().title(gettext("Service")).build();
    let start_stop = gtk::Button::builder()
        .valign(gtk::Align::Center)
        .css_classes(["flat", "circular"])
        .build();
    let restart = gtk::Button::builder()
        .icon_name("view-refresh-symbolic")
        .tooltip_text(gettext("Restart"))
        .valign(gtk::Align::Center)
        .css_classes(["flat", "circular"])
        .build();
    service.add_suffix(&start_stop);
    service.add_suffix(&restart);
    group.add(&service);

    // Soloist itself.
    let build = adw::ActionRow::builder()
        .title(gettext("Soloist Build"))
        .use_markup(false)
        .build();
    let update = gtk::Button::builder()
        .label(gettext("Update Now"))
        .valign(gtk::Align::Center)
        .css_classes(["flat"])
        .build();
    build.add_suffix(&update);
    group.add(&build);

    // Settings in its config.json.
    let websocket = adw::SwitchRow::builder()
        .title(gettext("Control by Gatefold"))
        .build();
    group.add(&websocket);
    let device = adw::EntryRow::builder()
        .title(gettext("Device Name"))
        .show_apply_button(true)
        .build();
    group.add(&device);
    let output = adw::EntryRow::builder()
        .title(gettext("PipeWire Output (empty: the default)"))
        .show_apply_button(true)
        .build();
    group.add(&output);
    let volume = adw::SpinRow::builder()
        .title(gettext("Volume at Start"))
        .adjustment(&gtk::Adjustment::new(100.0, 0.0, 100.0, 5.0, 10.0, 0.0))
        .build();
    let volume_apply = gtk::Button::builder()
        .icon_name("object-select-symbolic")
        .tooltip_text(gettext("Apply"))
        .valign(gtk::Align::Center)
        .css_classes(["flat", "circular"])
        .visible(false)
        .build();
    volume.add_suffix(&volume_apply);
    group.add(&volume);
    let key = adw::PasswordEntryRow::builder()
        .title(gettext("Change Soloist API Key"))
        .show_apply_button(true)
        .build();
    group.add(&key);

    let link = adw::ButtonRow::builder()
        .title(gettext("_Link Again…"))
        .use_underline(true)
        .start_icon_name("phone-symbolic")
        .build();
    group.add(&link);
    let log = adw::ButtonRow::builder()
        .title(gettext("Show _Log"))
        .use_underline(true)
        .start_icon_name("text-x-generic-symbolic")
        .build();
    group.add(&log);

    let configured_rows: Vec<gtk::Widget> = vec![
        websocket.clone().upcast(),
        device.clone().upcast(),
        output.clone().upcast(),
        volume.clone().upcast(),
        key.clone().upcast(),
        link.clone().upcast(),
    ];
    let installed_rows: Vec<gtk::Widget> = vec![
        service.clone().upcast(),
        build.clone().upcast(),
        log.clone().upcast(),
    ];
    // Filling the rows must not count as the user changing them.
    let filling = Rc::new(Cell::new(false));

    let refresh: Rc<dyn Fn()> = Rc::new(glib::clone!(
        #[weak]
        missing,
        #[weak]
        service,
        #[weak]
        start_stop,
        #[weak]
        build,
        #[weak]
        websocket,
        #[weak]
        device,
        #[weak]
        output,
        #[weak]
        volume,
        #[weak]
        volume_apply,
        #[strong]
        filling,
        move || {
            let configured_rows = configured_rows.clone();
            let installed_rows = installed_rows.clone();
            let filling = filling.clone();
            glib::spawn_future_local(async move {
                let Ok(state) = gio::spawn_blocking(snapshot).await else {
                    return;
                };
                let present = state.installed || state.config.is_some();
                missing.set_visible(!present);
                for row in &installed_rows {
                    row.set_visible(present);
                }
                for row in &configured_rows {
                    row.set_visible(state.config.is_some());
                }
                let running = state.service == ServiceState::Running;
                service.set_subtitle(&match state.service {
                    ServiceState::Running => gettext("Running"),
                    ServiceState::Starting => gettext("Starting…"),
                    ServiceState::Stopped => gettext("Stopped"),
                    ServiceState::Failed => gettext("Failed; see the log"),
                    ServiceState::NotInstalled => {
                        gettext("Not set up as a service: run “soloistd service install”")
                    }
                    ServiceState::Unknown => gettext("Unknown (no systemd user session)"),
                });
                let can_control = !matches!(
                    state.service,
                    ServiceState::NotInstalled | ServiceState::Unknown
                );
                start_stop.set_visible(can_control);
                start_stop.set_icon_name(if running {
                    "media-playback-stop-symbolic"
                } else {
                    "media-playback-start-symbolic"
                });
                start_stop.set_tooltip_text(Some(&if running {
                    gettext("Stop")
                } else {
                    gettext("Start")
                }));
                build.set_subtitle(
                    &state
                        .soloist
                        .clone()
                        .unwrap_or_else(|| gettext("Not downloaded yet")),
                );
                if let Some(config) = &state.config {
                    filling.set(true);
                    let address = config.websocket();
                    websocket.set_active(address.is_some());
                    websocket.set_subtitle(&match &address {
                        // Translators: {} is an address like 127.0.0.1:9090.
                        Some(address) => {
                            gettext("Soloist’s WebSocket API on {}").replace("{}", address)
                        }
                        None => gettext("Turns on Soloist’s WebSocket API (on this computer only)"),
                    });
                    device.set_text(&config.device_name().unwrap_or_default());
                    output.set_text(&config.string("pipewire_device").unwrap_or_default());
                    volume.set_value(config.number("initial_volume").unwrap_or(100) as f64);
                    volume_apply.set_visible(false);
                    filling.set(false);
                }
            });
        }
    ));
    refresh();

    // Changing a setting: write it, restart the service, use it again.
    let apply: Rc<dyn Fn(ConfigChange)> = Rc::new(glib::clone!(
        #[weak]
        spotify,
        #[strong]
        refresh,
        move |change: ConfigChange| {
            let refresh = refresh.clone();
            // Let go first: the restart closes the connection anyway.
            spotify.soloist_stop();
            glib::spawn_future_local(async move {
                let result = gio::spawn_blocking(move || {
                    let mut config = Config::load().ok_or("soloistd's config can't be read")?;
                    change(&mut config);
                    config.save().map_err(|e| e.to_string())?;
                    // Running or not, start it with the new settings.
                    match soloistd::service_state() {
                        ServiceState::Running
                        | ServiceState::Starting
                        | ServiceState::Stopped
                        | ServiceState::Failed => soloistd::control_service("restart"),
                        _ => Ok(()),
                    }
                })
                .await
                .unwrap_or_else(|_| Err("failed".into()));
                match result {
                    Ok(()) => {
                        spotify.message(&gettext("soloistd settings saved; Soloist restarted"));
                        reconnect(&spotify);
                    }
                    Err(err) => {
                        tracing::warn!("Cannot change soloistd's settings: {err}");
                        spotify.message(&gettext("soloistd’s settings couldn’t be changed"));
                    }
                }
                refresh();
            });
        }
    ));

    websocket.connect_active_notify(glib::clone!(
        #[strong]
        apply,
        #[strong]
        filling,
        move |row| {
            if filling.get() {
                return;
            }
            let on = row.is_active();
            apply(Box::new(move |config| {
                let address = config
                    .websocket()
                    .unwrap_or_else(|| soloistd::DEFAULT_WEBSOCKET.to_owned());
                config.set_string("websocket", on.then_some(address.as_str()));
            }));
        }
    ));
    device.connect_apply(glib::clone!(
        #[strong]
        apply,
        move |row| {
            let name = row.text().to_string();
            apply(Box::new(move |config| {
                config.set_string("device_name", Some(&name))
            }));
        }
    ));
    output.connect_apply(glib::clone!(
        #[strong]
        apply,
        move |row| {
            let node = row.text().to_string();
            apply(Box::new(move |config| {
                config.set_string("pipewire_device", Some(&node))
            }));
        }
    ));
    volume.connect_value_notify(glib::clone!(
        #[weak]
        volume_apply,
        #[strong]
        filling,
        move |_| {
            if !filling.get() {
                volume_apply.set_visible(true);
            }
        }
    ));
    volume_apply.connect_clicked(glib::clone!(
        #[strong]
        apply,
        #[weak]
        volume,
        move |button| {
            button.set_visible(false);
            let value = volume.value().round() as u64;
            apply(Box::new(move |config| {
                config.set_number("initial_volume", Some(value))
            }));
        }
    ));
    key.connect_apply(glib::clone!(
        #[strong]
        apply,
        move |row| {
            let text = row.text().trim().to_owned();
            row.set_text("");
            if text.is_empty() {
                return;
            }
            apply(Box::new(move |config| {
                config.set_string("api_key", Some(&text))
            }));
        }
    ));

    // The service's buttons.
    let control = glib::clone!(
        #[weak]
        spotify,
        #[strong]
        refresh,
        move |action: &'static str| {
            let refresh = refresh.clone();
            if action != "start" {
                spotify.soloist_stop();
            }
            glib::spawn_future_local(async move {
                let result = gio::spawn_blocking(move || soloistd::control_service(action))
                    .await
                    .unwrap_or_else(|_| Err("failed".into()));
                match result {
                    Ok(()) if action != "stop" => reconnect(&spotify),
                    Ok(()) => {}
                    Err(err) => {
                        tracing::warn!("soloistd service {action}: {err}");
                        spotify.message(&gettext("The soloistd service couldn’t be changed"));
                    }
                }
                refresh();
            });
        }
    );
    start_stop.connect_clicked(glib::clone!(
        #[strong]
        control,
        move |button| {
            let stopping = button.icon_name().as_deref() == Some("media-playback-stop-symbolic");
            control(if stopping { "stop" } else { "start" });
        }
    ));
    restart.connect_clicked(glib::clone!(
        #[strong]
        control,
        move |_| control("restart")
    ));

    update.connect_clicked(glib::clone!(
        #[weak]
        spotify,
        #[strong]
        refresh,
        move |button| {
            button.set_sensitive(false);
            let button = button.clone();
            let refresh = refresh.clone();
            glib::spawn_future_local(async move {
                spotify.message(&gettext("Updating Soloist…"));
                let result = gio::spawn_blocking(soloistd::update)
                    .await
                    .unwrap_or_else(|_| Err("failed".into()));
                button.set_sensitive(true);
                match result {
                    Ok(()) => {
                        spotify.message(&gettext("Soloist is up to date"));
                        reconnect(&spotify);
                    }
                    Err(err) => {
                        tracing::warn!("soloistd update: {err}");
                        spotify.message(&gettext("Soloist couldn’t be updated"));
                    }
                }
                refresh();
            });
        }
    ));

    link.connect_activated(glib::clone!(
        #[weak]
        spotify,
        #[weak]
        group,
        #[strong]
        refresh,
        move |_| {
            let refresh = refresh.clone();
            glib::spawn_future_local(async move {
                link_again(&spotify, &group).await;
                refresh();
            });
        }
    ));
    log.connect_activated(glib::clone!(
        #[weak]
        group,
        move |_| {
            glib::spawn_future_local(async move { show_log(&group).await });
        }
    ));
    group
}

/// After a restart, Gatefold attaches again (Soloist needs a moment).
fn reconnect(spotify: &Spotify) {
    if !spotify.settings().boolean("spotify-streaming") {
        return;
    }
    spotify.soloist_stop();
    glib::timeout_add_seconds_local_once(
        2,
        glib::clone!(
            #[weak]
            spotify,
            move || spotify.start_streaming()
        ),
    );
}

/// "soloistd pair": the service stops, Soloist waits to be chosen in the
/// Spotify app, then the service starts again.
async fn link_again(spotify: &Spotify, parent: &impl IsA<gtk::Widget>) {
    let Some(binary) = soloistd::binary() else {
        spotify.message(&gettext("soloistd isn’t installed"));
        return;
    };
    let device = Config::load()
        .and_then(|c| c.device_name())
        .unwrap_or_else(|| format!("soloistd@{}", glib::host_name()));
    spotify.soloist_stop();
    let _ = gio::spawn_blocking(|| soloistd::control_service("stop")).await;
    let process = match gio::Subprocess::newv(
        &[binary.as_os_str(), std::ffi::OsStr::new("pair")],
        gio::SubprocessFlags::STDOUT_SILENCE | gio::SubprocessFlags::STDERR_SILENCE,
    ) {
        Ok(process) => process,
        Err(err) => {
            tracing::warn!("Cannot run soloistd pair: {err}");
            spotify.message(&gettext("Linking couldn’t start"));
            let _ = gio::spawn_blocking(|| soloistd::control_service("start")).await;
            return;
        }
    };
    let dialog = adw::AlertDialog::builder()
        .heading(gettext("Link Soloist Again"))
        .body(
            // Translators: {} is a device name.
            gettext(
                "In the Spotify app on your phone or computer, open the device list and choose “{}”. This closes by itself once it’s linked.",
            )
            .replace("{}", &device),
        )
        .close_response("cancel")
        .build();
    dialog.add_responses(&[("cancel", &gettext("_Cancel"))]);
    let finished = Rc::new(Cell::new(None::<bool>));
    glib::spawn_future_local(glib::clone!(
        #[strong]
        process,
        #[strong]
        finished,
        #[weak]
        dialog,
        async move {
            let ok = process.wait_check_future().await.is_ok();
            finished.set(Some(ok));
            dialog.force_close();
        }
    ));
    dialog.choose_future(Some(parent)).await;
    if finished.get().is_none() {
        process.force_exit();
    }
    let _ = gio::spawn_blocking(|| soloistd::control_service("start")).await;
    match finished.get() {
        Some(true) => spotify.message(&gettext("Soloist is linked")),
        Some(false) => spotify.message(&gettext("Linking didn’t work; see the log")),
        None => {}
    }
    reconnect(spotify);
}

async fn show_log(parent: &impl IsA<gtk::Widget>) {
    let text = gio::spawn_blocking(|| soloistd::logs(200))
        .await
        .unwrap_or_default();
    let view = gtk::TextView::builder()
        .editable(false)
        .monospace(true)
        .wrap_mode(gtk::WrapMode::WordChar)
        .top_margin(6)
        .bottom_margin(6)
        .left_margin(6)
        .right_margin(6)
        .build();
    view.buffer().set_text(if text.trim().is_empty() {
        "—"
    } else {
        text.as_str()
    });
    let scrolled = gtk::ScrolledWindow::builder()
        .min_content_height(360)
        .min_content_width(560)
        .child(&view)
        .build();
    let dialog = adw::AlertDialog::builder()
        .heading(gettext("soloistd Log"))
        .extra_child(&scrolled)
        .close_response("close")
        .build();
    dialog.add_responses(&[("close", &gettext("_Close"))]);
    // The newest lines at the bottom, in view.
    let buffer = view.buffer();
    let end = buffer.end_iter();
    let mark = buffer.create_mark(None, &end, false);
    view.scroll_mark_onscreen(&mark);
    dialog.choose_future(Some(parent)).await;
}
