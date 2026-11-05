use std::cell::{OnceCell, RefCell};
use std::path::{Path, PathBuf};

use adw::prelude::*;
use adw::subclass::prelude::*;
use gatefold_core::library::folders::{FolderError, validate_new_folder};
use gettextrs::gettext;
use gtk::{gio, glib};

use crate::config::APP_ID;
use crate::library_folders;

mod imp {
    use super::*;

    #[derive(Debug, Default, gtk::CompositeTemplate)]
    #[template(resource = "/io/github/sobakah/Gatefold/ui/preferences_dialog.ui")]
    pub struct PreferencesDialog {
        #[template_child]
        pub folders_group: TemplateChild<adw::PreferencesGroup>,
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
            self.settings.set(settings).expect("settings are set once");
            obj.refresh_folders();
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
        let row = adw::ActionRow::builder()
            .title(name)
            .subtitle(folder.display().to_string())
            .use_markup(false)
            .build();
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
