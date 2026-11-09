use std::cell::{OnceCell, RefCell};
use std::path::{Path, PathBuf};

use adw::prelude::*;
use adw::subclass::prelude::*;
use gatefold_core::library::folders::{FolderError, validate_new_folder};
use gettextrs::gettext;
use gtk::{gio, glib};

use crate::config::APP_ID;
use crate::library_folders;

/// The `replaygain` setting values, in the order of the combo row.
const REPLAYGAIN_MODES: [&str; 4] = ["off", "auto", "track", "album"];
/// The `tag-editor` setting values, in the order of the combo row.
const TAG_EDITORS: [&str; 5] = ["auto", "eartag", "kid3", "picard", "custom"];
/// The `lyrics-layout` setting values, in the order of the combo row.
/// "analysis-mode" values, in the order of the Analyze Songs row.
const ANALYSIS_MODES: [&str; 3] = ["off", "background", "full"];
const LYRICS_LAYOUTS: [&str; 3] = ["original-top", "romanization-top", "original-only"];
/// The "start-view" choices, in the row's order.
const START_VIEWS: [&str; 4] = ["last", "albums", "artists", "playlists"];
/// The "lyrics-pane" and "queue-pane" choices, in the rows' order.
const PANE_MODES: [&str; 3] = ["remember", "open", "closed"];

mod imp {
    use super::*;

    #[derive(Debug, Default, gtk::CompositeTemplate)]
    #[template(resource = "/io/github/sobakah/Gatefold/ui/preferences_dialog.ui")]
    pub struct PreferencesDialog {
        #[template_child]
        pub folders_group: TemplateChild<adw::PreferencesGroup>,
        #[template_child]
        pub replaygain_row: TemplateChild<adw::ComboRow>,
        #[template_child]
        pub crossfade_row: TemplateChild<adw::SpinRow>,
        #[template_child]
        pub crossfade_albums_row: TemplateChild<adw::SwitchRow>,
        #[template_child]
        pub crossfade_smart_row: TemplateChild<adw::SwitchRow>,
        #[template_child]
        pub restore_queue_row: TemplateChild<adw::SwitchRow>,
        #[template_child]
        pub stats_row: TemplateChild<adw::SwitchRow>,
        #[template_child]
        pub visualizer_row: TemplateChild<adw::SwitchRow>,
        #[template_child]
        pub delete_stats_row: TemplateChild<adw::ButtonRow>,
        #[template_child]
        pub lyrics_layout_row: TemplateChild<adw::ComboRow>,
        #[template_child]
        pub start_view_row: TemplateChild<adw::ComboRow>,
        #[template_child]
        pub style_row: TemplateChild<adw::ComboRow>,
        #[template_child]
        pub lyrics_pane_row: TemplateChild<adw::ComboRow>,
        #[template_child]
        pub queue_pane_row: TemplateChild<adw::ComboRow>,
        #[template_child]
        pub online_row: TemplateChild<adw::SwitchRow>,
        #[template_child]
        pub lyrembed_status: TemplateChild<adw::ActionRow>,
        #[template_child]
        pub romanize_status: TemplateChild<adw::ActionRow>,
        #[template_child]
        pub layout_hint: TemplateChild<adw::ActionRow>,
        #[template_child]
        pub lyrembed_path_row: TemplateChild<adw::EntryRow>,
        #[template_child]
        pub cdimport_status: TemplateChild<adw::ActionRow>,
        #[template_child]
        pub cdimport_library: TemplateChild<adw::ActionRow>,
        #[template_child]
        pub cdimport_path_row: TemplateChild<adw::EntryRow>,
        #[template_child]
        pub tag_editor_row: TemplateChild<adw::ComboRow>,
        #[template_child]
        pub tag_editor_command_row: TemplateChild<adw::EntryRow>,
        #[template_child]
        pub ear_tag_status: TemplateChild<adw::ActionRow>,
        #[template_child]
        pub wikidata_row: TemplateChild<adw::SwitchRow>,
        #[template_child]
        pub spotify_photos_row: TemplateChild<adw::SwitchRow>,
        #[template_child]
        pub deezer_row: TemplateChild<adw::SwitchRow>,
        #[template_child]
        pub lrclib_row: TemplateChild<adw::SwitchRow>,
        #[template_child]
        pub updates_row: TemplateChild<adw::SwitchRow>,
        #[template_child]
        pub spotify_app_row: TemplateChild<adw::SwitchRow>,
        #[template_child]
        pub analysis_row: TemplateChild<adw::ComboRow>,
        #[template_child]
        pub analysis_progress_row: TemplateChild<adw::ActionRow>,
        #[template_child]
        pub analysis_bar: TemplateChild<gtk::ProgressBar>,
        #[template_child]
        pub analysis_pause: TemplateChild<gtk::Button>,
        pub folder_rows: RefCell<Vec<adw::ActionRow>>,
        pub settings: OnceCell<gio::Settings>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for PreferencesDialog {
        const NAME: &'static str = "GatefoldPreferencesDialog";
        type Type = super::PreferencesDialog;
        type ParentType = adw::PreferencesDialog;

        fn class_init(klass: &mut Self::Class) {
            klass.bind_template();
            klass.install_action_async("prefs.add-folder", None, |dialog, _, _| async move {
                dialog.add_folder().await;
            });
        }

        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for PreferencesDialog {
        fn constructed(&self) {
            self.parent_constructed();
            let obj = self.obj();

            let settings = gio::Settings::new(APP_ID);
            settings.connect_changed(
                Some("library-folders"),
                glib::clone!(
                    #[weak]
                    obj,
                    move |_, _| obj.refresh_folders()
                ),
            );
            settings
                .bind("restore-queue", &*self.restore_queue_row, "active")
                .build();
            settings
                .bind("listening-stats", &*self.stats_row, "active")
                .build();
            settings
                .bind("visualizer", &*self.visualizer_row, "active")
                .build();
            self.delete_stats_row.connect_activated(glib::clone!(
                #[weak]
                obj,
                move |_| obj.ask_delete_stats()
            ));
            bind_choice(&settings, "start-view", &self.start_view_row, &START_VIEWS);
            bind_choice(&settings, "style", &self.style_row, &crate::style::STYLES);
            bind_choice(&settings, "lyrics-pane", &self.lyrics_pane_row, &PANE_MODES);
            bind_choice(&settings, "queue-pane", &self.queue_pane_row, &PANE_MODES);
            settings
                .bind("lyrics-layout", &*self.lyrics_layout_row, "selected")
                .mapping(|value, _| {
                    let layout = value.get::<String>()?;
                    let index = LYRICS_LAYOUTS.iter().position(|l| *l == layout)?;
                    Some((index as u32).to_value())
                })
                .set_mapping(|value, _| {
                    let index = value.get::<u32>().ok()? as usize;
                    LYRICS_LAYOUTS.get(index).map(|l| l.to_variant())
                })
                .build();
            settings
                .bind("online-metadata", &*self.online_row, "active")
                .build();
            settings
                .bind("metadata-wikidata", &*self.wikidata_row, "active")
                .build();
            settings
                .bind("metadata-spotify", &*self.spotify_photos_row, "active")
                .build();
            settings
                .bind("metadata-deezer", &*self.deezer_row, "active")
                .build();
            settings
                .bind("lyrics-lrclib", &*self.lrclib_row, "active")
                .build();
            settings
                .bind("check-updates", &*self.updates_row, "active")
                .build();
            settings
                .bind("spotify-links-in-app", &*self.spotify_app_row, "active")
                .build();
            self.setup_analysis(&settings);
            self.setup_integrations(&settings);
            // Answering here counts as answering the Artists view's question.
            self.online_row.connect_active_notify(glib::clone!(
                #[weak]
                settings,
                move |_| {
                    let _ = settings.set_boolean("online-metadata-asked", true);
                }
            ));
            settings
                .bind("replaygain", &*self.replaygain_row, "selected")
                .mapping(|value, _| {
                    let mode = value.get::<String>()?;
                    let index = REPLAYGAIN_MODES.iter().position(|m| *m == mode)?;
                    Some((index as u32).to_value())
                })
                .set_mapping(|value, _| {
                    let index = value.get::<u32>().ok()? as usize;
                    REPLAYGAIN_MODES.get(index).map(|m| m.to_variant())
                })
                .build();
            settings
                .bind("crossfade", &*self.crossfade_row, "value")
                .build();
            settings
                .bind("crossfade-albums", &*self.crossfade_albums_row, "active")
                .build();
            settings
                .bind("crossfade-smart", &*self.crossfade_smart_row, "active")
                .build();
            for row in [&*self.crossfade_albums_row, &*self.crossfade_smart_row] {
                self.crossfade_row
                    .bind_property("value", row, "sensitive")
                    .transform_to(|_, value: f64| Some(value > 0.0))
                    .sync_create()
                    .build();
            }
            self.settings.set(settings).expect("settings are set once");
            obj.refresh_folders();
            obj.add(&crate::spotify::preferences::page());
        }
    }

    impl PreferencesDialog {
        /// Song analysis: on/off, progress, pause (§7).
        fn setup_analysis(&self, settings: &gio::Settings) {
            settings
                .bind("analysis-mode", &*self.analysis_row, "selected")
                .mapping(|value, _| {
                    let index = ANALYSIS_MODES
                        .iter()
                        .position(|m| Some(*m) == value.str())
                        .unwrap_or(0);
                    Some((index as u32).to_value())
                })
                .set_mapping(|value, _| {
                    let index = value.get::<u32>().ok()? as usize;
                    ANALYSIS_MODES.get(index).map(|m| m.to_variant())
                })
                .build();
            let analyzer = crate::application::GatefoldApplication::get().analyzer();
            let row = self.analysis_progress_row.get();
            let bar = self.analysis_bar.get();
            let pause = self.analysis_pause.get();
            let update = glib::clone!(
                #[weak]
                row,
                #[weak]
                bar,
                #[weak]
                pause,
                #[strong]
                settings,
                move |analyzer: &crate::analyzer::Analyzer| {
                    let (done, total) = (analyzer.analyzed(), analyzer.total());
                    bar.set_fraction(if total > 0 {
                        f64::from(done) / f64::from(total)
                    } else {
                        0.0
                    });
                    // Translators: {done} and {total} are song counts.
                    row.set_title(
                        &gettext("{done} of {total} songs analyzed")
                            .replace("{done}", &done.to_string())
                            .replace("{total}", &total.to_string()),
                    );
                    let status = if total > 0 && done >= total {
                        gettext("Done")
                    } else if settings.string("analysis-mode") == "off" {
                        gettext("Off")
                    } else if analyzer.paused() {
                        gettext("Paused")
                    } else if analyzer.on_battery() {
                        gettext("Waiting while on battery")
                    } else if analyzer.busy() {
                        gettext("Analyzing…")
                    } else {
                        gettext("Waiting")
                    };
                    row.set_subtitle(&status);
                    let paused = analyzer.paused();
                    pause.set_icon_name(if paused {
                        "media-playback-start-symbolic"
                    } else {
                        "media-playback-pause-symbolic"
                    });
                    pause.set_tooltip_text(Some(&if paused {
                        gettext("Resume")
                    } else {
                        gettext("Pause")
                    }));
                    pause.set_visible(done < total && settings.string("analysis-mode") != "off");
                }
            );
            update(&analyzer);
            // Disconnected when the dialog goes (the setting's handler holds
            // the settings themselves, which would stay forever).
            let obj = self.obj();
            for property in ["analyzed", "total", "busy", "paused", "on-battery"] {
                let update = update.clone();
                let id = analyzer
                    .connect_notify_local(Some(property), move |analyzer, _| update(analyzer));
                crate::widgets::disconnect_with(&*obj, &analyzer, id);
            }
            let on_setting = update.clone();
            let analyzer_for_setting = analyzer.clone();
            let id = settings.connect_changed(Some("analysis-mode"), move |_, _| {
                on_setting(&analyzer_for_setting)
            });
            crate::widgets::disconnect_with(&*obj, settings, id);
            pause.connect_clicked(move |_| {
                analyzer.set_paused(!analyzer.paused());
                update(&analyzer);
            });
        }

        fn setup_integrations(&self, settings: &gio::Settings) {
            // Paths apply when the entry is confirmed (Enter), not per key.
            for (row, key) in [
                (&*self.lyrembed_path_row, "lyrembed-path"),
                (&*self.cdimport_path_row, "cdimport-path"),
                (&*self.tag_editor_command_row, "tag-editor-command"),
            ] {
                row.set_text(&settings.string(key));
                row.set_show_apply_button(true);
                row.connect_apply(glib::clone!(
                    #[weak]
                    settings,
                    move |row| {
                        let _ = settings.set_string(key, row.text().trim());
                    }
                ));
            }
            settings
                .bind("tag-editor", &*self.tag_editor_row, "selected")
                .mapping(|value, _| {
                    let editor = value.get::<String>()?;
                    let index = TAG_EDITORS.iter().position(|e| *e == editor)?;
                    Some((index as u32).to_value())
                })
                .set_mapping(|value, _| {
                    let index = value.get::<u32>().ok()? as usize;
                    TAG_EDITORS.get(index).map(|e| e.to_variant())
                })
                .build();
            self.tag_editor_row
                .bind_property("selected", &*self.tag_editor_command_row, "sensitive")
                .transform_to(|_, selected: u32| Some(selected == 4))
                .sync_create()
                .build();

            let obj = self.obj();
            let tools = crate::application::GatefoldApplication::get().tools();
            let detected = tools.connect_local(
                "detected",
                false,
                glib::clone!(
                    #[weak]
                    obj,
                    #[upgrade_or]
                    None,
                    move |_| {
                        obj.show_tools();
                        None
                    }
                ),
            );
            crate::widgets::disconnect_with(&*obj, &tools, detected);
            obj.show_tools();
        }
    }

    impl WidgetImpl for PreferencesDialog {}
    impl AdwDialogImpl for PreferencesDialog {}
    impl PreferencesDialogImpl for PreferencesDialog {}
}

glib::wrapper! {
    pub struct PreferencesDialog(ObjectSubclass<imp::PreferencesDialog>)
        @extends gtk::Widget, adw::Dialog, adw::PreferencesDialog,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget, gtk::ShortcutManager;
}

impl PreferencesDialog {
    pub fn new() -> Self {
        glib::Object::new()
    }

    fn settings(&self) -> &gio::Settings {
        self.imp()
            .settings
            .get()
            .expect("settings are set in constructed()")
    }

    /// Deletes the play log after asking (the history stays).
    fn ask_delete_stats(&self) {
        let dialog = adw::AlertDialog::new(
            Some(&gettext("Delete Listening Statistics?")),
            Some(&gettext(
                "Everything logged so far is deleted; they can't be brought back. Recently Played stays.",
            )),
        );
        dialog.add_responses(&[
            ("cancel", &gettext("_Cancel")),
            ("delete", &gettext("_Delete")),
        ]);
        dialog.set_response_appearance("delete", adw::ResponseAppearance::Destructive);
        dialog.set_default_response(Some("cancel"));
        dialog.set_close_response("cancel");
        dialog.connect_response(
            Some("delete"),
            glib::clone!(
                #[weak(rename_to = preferences)]
                self,
                move |_, _| {
                    let library = crate::application::GatefoldApplication::get().library();
                    let deleted = library.db().map(|mut db| db.delete_play_log());
                    match deleted {
                        Some(Ok(())) => {
                            preferences.add_toast(adw::Toast::new(&gettext("Statistics deleted")));
                        }
                        Some(Err(err)) => {
                            tracing::warn!("Cannot delete the statistics: {err}");
                            preferences.add_toast(adw::Toast::new(&gettext(
                                "The statistics couldn't be deleted",
                            )));
                        }
                        None => {}
                    }
                }
            ),
        );
        dialog.present(Some(self));
    }

    fn refresh_folders(&self) {
        let imp = self.imp();
        for row in imp.folder_rows.take() {
            imp.folders_group.remove(&row);
        }

        let folders = library_folders::load(self.settings());
        let rows: Vec<adw::ActionRow> = if folders.is_empty() {
            vec![
                adw::ActionRow::builder()
                    .title(gettext("No music folders"))
                    .css_classes(["dim-label"])
                    .build(),
            ]
        } else {
            folders
                .iter()
                .map(|folder| self.folder_row(folder))
                .collect()
        };
        for row in &rows {
            imp.folders_group.add(row);
        }
        imp.folder_rows.replace(rows);
    }

    fn folder_row(&self, folder: &Path) -> adw::ActionRow {
        let name = folder.file_name().map_or_else(
            || folder.display().to_string(),
            |name| name.to_string_lossy().into_owned(),
        );
        let row = {
            let row = adw::ActionRow::builder().use_markup(false).build();
            row.set_title(&name);
            row.set_subtitle(&folder.display().to_string());
            row
        };
        row.add_prefix(&gtk::Image::from_icon_name("folder-music-symbolic"));

        let remove = gtk::Button::builder()
            .icon_name("user-trash-symbolic")
            .tooltip_text(gettext("Remove"))
            .valign(gtk::Align::Center)
            .css_classes(["flat"])
            .build();
        let folder = folder.to_path_buf();
        remove.connect_clicked(glib::clone!(
            #[weak(rename_to = dialog)]
            self,
            move |_| dialog.remove_folder(&folder)
        ));
        row.add_suffix(&remove);
        row
    }

    async fn add_folder(&self) {
        let file_dialog = gtk::FileDialog::builder()
            .title(gettext("Add Music Folder"))
            .modal(true)
            .build();
        let parent = self.root().and_downcast::<gtk::Window>();
        let file = match file_dialog.select_folder_future(parent.as_ref()).await {
            Ok(file) => file,
            Err(err) => {
                if !err.matches(gtk::DialogError::Dismissed) {
                    tracing::warn!("Folder chooser failed: {err}");
                }
                return;
            }
        };
        let Some(candidate) = file.path() else {
            self.toast(&gettext("Only local folders can be library folders"));
            return;
        };

        let mut folders = library_folders::load(self.settings());
        let home = glib::home_dir();
        match validate_new_folder(&candidate, &folders, Some(home.as_path())) {
            Ok(folder) => {
                folders.push(folder);
                self.store(&folders);
            }
            Err(err) => {
                tracing::info!("Refused library folder {}: {err}", candidate.display());
                self.toast(&folder_error_message(&err));
            }
        }
    }

    fn remove_folder(&self, folder: &Path) {
        let folders: Vec<PathBuf> = library_folders::load(self.settings())
            .into_iter()
            .filter(|f| f != folder)
            .collect();
        self.store(&folders);
    }

    fn store(&self, folders: &[PathBuf]) {
        if let Err(err) = library_folders::store(self.settings(), folders) {
            tracing::warn!("Failed to save library folders: {err}");
        }
    }

    /// Found / version / path of each tool, or how to install it (§10.0).
    fn show_tools(&self) {
        use gatefold_tools::{cdimport, lyrembed};
        let imp = self.imp();
        let detected = crate::application::GatefoldApplication::get()
            .tools()
            .detected();
        let describe = |tool: &Option<crate::tools::ToolInfo>, install: &str| match tool {
            Some(tool) => match tool.version {
                // Translators: {version} and {path} of a found program.
                Some(version) => gettext("Version {version} · {path}")
                    .replace("{version}", &version.to_string())
                    .replace("{path}", &tool.path.display().to_string()),
                None => tool.path.display().to_string(),
            },
            // Translators: {} is an install command, e.g. "pipx install lyrembed".
            None => gettext("Not installed. Install it with “{}”").replace("{}", install),
        };
        imp.lyrembed_status
            .set_subtitle(&describe(&detected.lyrembed, lyrembed::INSTALL_COMMAND));
        imp.cdimport_status
            .set_subtitle(&describe(&detected.cdimport, cdimport::INSTALL_COMMAND));

        let romanize = if detected.romanize_stdio {
            gettext("Lyrics without romanization get it for display, also from LRCLIB")
        } else {
            // §5.1: without "romanize --stdio", lyrics show as tagged.
            gettext(
                "Not available: needs lyrembed 1.1.1 or newer (update it on lyrembed's page, or with “pipx upgrade lyrembed”). Lyrics show as tagged.",
            )
        };
        imp.romanize_status.set_subtitle(&romanize);
        imp.layout_hint
            .set_visible(detected.lyrembed.is_some() && !lyrembed::Config::read().keeps_original());

        let library = cdimport::Config::read().library_dir;
        imp.cdimport_library
            .set_visible(detected.cdimport.is_some());
        imp.cdimport_library.set_subtitle(
            &library
                .map(|dir| dir.display().to_string())
                .unwrap_or_else(|| gettext("Asked on first use")),
        );
        imp.ear_tag_status.set_subtitle(&if detected.ear_tag {
            gettext("Installed")
        } else {
            gettext("Not installed. Get it from GNOME Software.")
        });
    }

    fn toast(&self, message: &str) {
        self.add_toast(adw::Toast::new(message));
    }
}

impl Default for PreferencesDialog {
    fn default() -> Self {
        Self::new()
    }
}

fn folder_error_message(err: &FolderError) -> String {
    let path = |p: &Path| p.display().to_string();
    match err {
        FolderError::NotFound => gettext("The folder doesn't exist"),
        FolderError::NotADirectory => gettext("That's not a folder"),
        FolderError::NotUtf8 => gettext("Folder names must be valid UTF-8"),
        FolderError::Symlink { .. } => gettext("Symlinked folders can't be library folders"),
        FolderError::AlreadyAdded => gettext("This folder is already in the library"),
        // Translators: {} is a folder path.
        FolderError::InsideExisting(p) => {
            gettext("This folder is already part of “{}”").replace("{}", &path(p))
        }
        // Translators: {} is a folder path.
        FolderError::ContainsExisting(p) => {
            gettext("This folder contains the library folder “{}”").replace("{}", &path(p))
        }
        FolderError::Io(_) => gettext("The folder can't be read"),
    }
}

/// A string setting with choices shown as a combo row (`values` in the
/// row's order).
fn bind_choice(
    settings: &gio::Settings,
    key: &str,
    row: &adw::ComboRow,
    values: &'static [&'static str],
) {
    settings
        .bind(key, row, "selected")
        .mapping(move |value, _| {
            let value = value.get::<String>()?;
            let index = values.iter().position(|v| *v == value)?;
            Some((index as u32).to_value())
        })
        .set_mapping(move |value, _| {
            let index = value.get::<u32>().ok()? as usize;
            values.get(index).map(|v| v.to_variant())
        })
        .build();
}
