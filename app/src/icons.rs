//! Jewelcase's icons (data/resources/icons): every symbolic icon it uses is
//! bundled, so icon themes that lack one (KDE's Breeze lacks several)
//! don't show a missing icon. The desktop's theme still wins where it has
//! the icon.

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;
    use std::path::{Path, PathBuf};

    /// Built into GTK or libadwaita, so always there.
    const BUILT_IN: [&str; 2] = ["adw-external-link-symbolic", "info-outline-symbolic"];

    fn repo() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("..")
    }

    fn files(dir: &Path, extension: &str, found: &mut Vec<PathBuf>) {
        for entry in std::fs::read_dir(dir).unwrap().flatten() {
            let path = entry.path();
            if path.is_dir() {
                files(&path, extension, found);
            } else if path.extension().is_some_and(|e| e == extension) {
                found.push(path);
            }
        }
    }

    /// Every quoted `…-symbolic` name in the code and the UI files.
    fn used() -> BTreeSet<String> {
        let mut sources = Vec::new();
        for dir in ["app/src", "core/src", "tools/src"] {
            files(&repo().join(dir), "rs", &mut sources);
        }
        files(&repo().join("data/resources/ui"), "blp", &mut sources);
        let mut names = BTreeSet::new();
        for source in sources {
            let text = std::fs::read_to_string(&source).unwrap();
            for part in text.split('"').skip(1).step_by(2) {
                let is_name = part.ends_with("-symbolic")
                    && part
                        .chars()
                        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-');
                if is_name {
                    names.insert(part.to_owned());
                }
            }
        }
        names
    }

    #[test]
    fn every_used_icon_is_bundled() {
        let xml =
            std::fs::read_to_string(repo().join("data/resources/resources.gresource.xml")).unwrap();
        let dir = repo().join("data/resources/icons/scalable/actions");
        let missing: Vec<_> = used()
            .into_iter()
            .filter(|name| !BUILT_IN.contains(&name.as_str()))
            .filter(|name| {
                let file = format!("icons/scalable/actions/{name}.svg");
                !(xml.contains(&format!(">{file}<")) && dir.join(format!("{name}.svg")).exists())
            })
            .collect();
        assert!(
            missing.is_empty(),
            "icons used but not bundled (copy them from adwaita-icon-theme into \
             data/resources/icons/scalable/actions and list them in \
             resources.gresource.xml): {missing:?}"
        );
    }
}
