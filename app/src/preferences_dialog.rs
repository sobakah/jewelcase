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
        pub lyrembed_setup: TemplateChild<adw::PreferencesGroup>,
        #[template_child]
        pub lyrembed_steps: TemplateChild<gtk::ListBox>,
        #[template_child]
        pub cdport_status: TemplateChild<adw::ActionRow>,
        #[template_child]
        pub cdport_library: TemplateChild<adw::ActionRow>,
        #[template_child]
        pub cdport_path_row: TemplateChild<adw::EntryRow>,
        #[template_child]
        pub cdport_setup: TemplateChild<adw::PreferencesGroup>,
        #[template_child]
        pub cdport_steps: TemplateChild<gtk::ListBox>,
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
                (&*self.cdport_path_row, "cdport-path"),
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
            // Looked for again: installed or set up since the last look.
            tools.detect();
            // A Flatpak doesn't see the programs: only D-Bus counts there.
            let flatpak = crate::config::is_flatpak();
            self.lyrembed_path_row.set_visible(!flatpak);
            self.cdport_path_row.set_visible(!flatpak);
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

    /// Found / version / path of each tool, or the steps to install and
    /// set it up (§10.0).
    fn show_tools(&self) {
        use gatefold_tools::lyrembed;
        let imp = self.imp();
        let detected = crate::application::GatefoldApplication::get()
            .tools()
            .detected();
        let flatpak = crate::config::is_flatpak();

        // lyrembed
        let lyrembed = detected.lyrembed.as_ref();
        imp.lyrembed_status.set_subtitle(&match lyrembed {
            Some(tool) => found(tool.version, Some(&tool.path)),
            None if flatpak => gettext("Not available in the Flatpak version yet"),
            None => gettext("Not installed"),
        });
        imp.romanize_status.set_visible(lyrembed.is_some());
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
            .set_visible(lyrembed.is_some() && !lyrembed::Config::read().keeps_original());
        let setup = if lyrembed.is_some() {
            None
        } else if flatpak {
            Some(Setup {
                title: gettext("lyrembed and the Flatpak"),
                description: gettext(
                    "A Flatpak can't start programs on your computer. lyrembed will get a D-Bus interface for this, as cdport has; until then, its lyrics features are only in the other versions of Gatefold.",
                ),
                steps: Vec::new(),
            })
        } else {
            Some(Setup {
                title: gettext("Install lyrembed"),
                description: gettext(
                    "In a terminal. Gatefold finds lyrembed by itself afterwards, or press Look Again above.",
                ),
                steps: [
                    pipx_steps(),
                    vec![Step::command(
                        gettext("Install lyrembed, with romanization"),
                        lyrembed::INSTALL_COMMAND,
                    )],
                ]
                .concat(),
            })
        };
        self.show_setup(&imp.lyrembed_setup, &imp.lyrembed_steps, setup);

        // cdport
        let cdport = &detected.cdport;
        imp.cdport_status
            .set_subtitle(&cdport_status(cdport, flatpak));
        imp.cdport_library.set_visible(cdport.usable());
        imp.cdport_library.set_subtitle(
            &cdport
                .config
                .library_dir
                .as_ref()
                .map(|dir| dir.display().to_string())
                .unwrap_or_else(|| gettext("Asked on first use")),
        );
        self.show_setup(
            &imp.cdport_setup,
            &imp.cdport_steps,
            cdport_setup(cdport, flatpak),
        );

        imp.ear_tag_status.set_subtitle(&if detected.ear_tag {
            gettext("Installed")
        } else {
            gettext("Not installed. Get it from GNOME Software.")
        });
    }

    /// Fills a setup group (hidden without one).
    fn show_setup(&self, group: &adw::PreferencesGroup, list: &gtk::ListBox, setup: Option<Setup>) {
        list.remove_all();
        let Some(setup) = setup else {
            group.set_visible(false);
            return;
        };
        group.set_title(&setup.title);
        group.set_description(Some(&setup.description));
        list.set_visible(!setup.steps.is_empty());
        let numbered = setup.steps.len() > 1;
        for (index, step) in setup.steps.into_iter().enumerate() {
            let title = if numbered {
                format!("{}. {}", index + 1, step.text)
            } else {
                step.text
            };
            let row = adw::ActionRow::builder()
                .title(glib::markup_escape_text(&title))
                .subtitle_selectable(true)
                .build();
            if let Some(command) = step.command {
                row.set_subtitle(&format!("<tt>{}</tt>", glib::markup_escape_text(&command)));
                let copy = gtk::Button::builder()
                    .icon_name("edit-copy-symbolic")
                    .tooltip_text(gettext("Copy"))
                    .valign(gtk::Align::Center)
                    .css_classes(["flat"])
                    .build();
                copy.connect_clicked(glib::clone!(
                    #[weak(rename_to = dialog)]
                    self,
                    move |button| {
                        button.clipboard().set_text(&command);
                        dialog.toast(&gettext("Copied"));
                    }
                ));
                row.add_suffix(&copy);
            }
            if let Some((label, action)) = step.button {
                let button = gtk::Button::builder()
                    .label(label)
                    .use_underline(true)
                    .action_name(action)
                    .valign(gtk::Align::Center)
                    .css_classes(["suggested-action"])
                    .build();
                row.add_suffix(&button);
            }
            list.append(&row);
        }
        group.set_visible(true);
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

/// A setup group: what to do so Gatefold can use a tool.
struct Setup {
    title: String,
    description: String,
    steps: Vec<Step>,
}

/// One step: what it does, the command to run (copyable), and a button
/// that does it (an action).
#[derive(Clone)]
struct Step {
    text: String,
    command: Option<String>,
    button: Option<(String, &'static str)>,
}

impl Step {
    fn command(text: String, command: &str) -> Self {
        Self {
            text,
            command: Some(command.to_owned()),
            button: None,
        }
    }
}

/// "Version 1.3.0 · /home/…/cdport", or less when unknown.
fn found(version: Option<gatefold_tools::Version>, path: Option<&Path>) -> String {
    match (version, path) {
        // Translators: {version} and {path} of a found program.
        (Some(version), Some(path)) => gettext("Version {version} · {path}")
            .replace("{version}", &version.to_string())
            .replace("{path}", &path.display().to_string()),
        // Translators: {} is a version number.
        (Some(version), None) => gettext("Version {}").replace("{}", &version.to_string()),
        (None, Some(path)) => path.display().to_string(),
        (None, None) => gettext("Found"),
    }
}

/// Installing pipx and putting its programs on PATH (the first steps).
fn pipx_steps() -> Vec<Step> {
    use gatefold_tools::{PIPX_INSTALL_COMMAND, PIPX_PATH_COMMAND};
    vec![
        Step::command(
            gettext("Install pipx (on Fedora; other distributions have a pipx package too)"),
            PIPX_INSTALL_COMMAND,
        ),
        Step::command(
            gettext(
                "Let the terminal find the programs pipx installs (once, then open a new terminal)",
            ),
            PIPX_PATH_COMMAND,
        ),
    ]
}

fn cdport_status(cdport: &crate::tools::Cdport, flatpak: bool) -> String {
    let path = cdport.program.as_ref().map(|p| p.path.as_path());
    if cdport.dbus {
        // Translators: {} is "Version 1.3.0 · <path>" or "Version 1.3.0".
        return gettext("{} · through D-Bus").replace("{}", &found(cdport.version, path));
    }
    if let Some(error) = &cdport.dbus_error {
        // Translators: {} is the error message.
        return gettext("Its D-Bus service doesn't start: {}").replace("{}", error);
    }
    if cdport.is_old() {
        // Translators: {} is "Version 1.2.1 · <path>".
        return gettext("cdimport, cdport’s old version: {}")
            .replace("{}", &found(cdport.version, path));
    }
    if cdport.program.is_some() {
        return found(cdport.version, path);
    }
    if flatpak {
        gettext("Not found: not installed, or its D-Bus service isn't set up")
    } else {
        gettext("Not installed")
    }
}

fn cdport_setup(cdport: &crate::tools::Cdport, flatpak: bool) -> Option<Setup> {
    use gatefold_tools::cdport::{INSTALL_COMMAND, REMOVE_OLD_COMMAND, SERVICE_COMMAND};
    let service_step = || {
        Step::command(
            gettext("Let Gatefold reach cdport through D-Bus"),
            SERVICE_COMMAND,
        )
    };
    if cdport.dbus {
        return None;
    }
    if cdport.dbus_error.is_some() {
        let mut steps = Vec::new();
        if cdport.program.is_none() {
            steps.push(Step::command(
                gettext("If cdport was removed, install it again"),
                INSTALL_COMMAND,
            ));
        }
        steps.push(set_up_step(
            cdport,
            gettext("Set up its D-Bus service again"),
        ));
        return Some(Setup {
            title: gettext("Repair cdport’s D-Bus Service"),
            description: gettext(
                "The service is set up, but cdport didn't start through it, e.g. because cdport was moved or removed since.",
            ),
            steps,
        });
    }
    if cdport.is_old() {
        return Some(Setup {
            title: gettext("Update to cdport"),
            description: gettext(
                "cdimport is now called cdport. From version 1.3.0 on, it has a D-Bus interface: imports keep running when Gatefold closes, and the Flatpak version of Gatefold can use it too. Until you update, Gatefold uses cdimport as before.",
            ),
            steps: vec![
                Step::command(
                    gettext("Remove cdimport (your settings stay)"),
                    REMOVE_OLD_COMMAND,
                ),
                Step::command(gettext("Install cdport"), INSTALL_COMMAND),
                service_step(),
            ],
        });
    }
    if cdport.can_set_up_service() {
        return Some(Setup {
            title: gettext("Set Up cdport’s D-Bus Service"),
            description: gettext(
                "Recommended. Gatefold starts cdport directly now. Through D-Bus, imports keep running when Gatefold closes, and the Flatpak version of Gatefold can use cdport too.",
            ),
            steps: vec![set_up_step(cdport, gettext("Set up the D-Bus service"))],
        });
    }
    let description = if flatpak {
        gettext(
            "In a terminal on your computer (not inside Gatefold). Gatefold, as a Flatpak, reaches cdport through its D-Bus service, the last step. cdport offers to install cyanrip, which it rips with, when it first needs it.",
        )
    } else {
        gettext(
            "In a terminal. cdport offers to install cyanrip, which it rips with, when it first needs it. Gatefold finds cdport by itself afterwards, or press Look Again above.",
        )
    };
    Some(Setup {
        title: gettext("Install cdport"),
        description,
        steps: [
            pipx_steps(),
            vec![
                Step::command(gettext("Install cdport"), INSTALL_COMMAND),
                service_step(),
            ],
        ]
        .concat(),
    })
}

/// `cdport --install-dbus-service`, with a button where Gatefold can run
/// it (the program is visible: not from a Flatpak).
fn set_up_step(cdport: &crate::tools::Cdport, text: String) -> Step {
    Step {
        text,
        command: Some(gatefold_tools::cdport::SERVICE_COMMAND.to_owned()),
        button: cdport
            .program
            .is_some()
            .then(|| (gettext("_Set Up"), "app.set-up-cdport")),
    }
}
