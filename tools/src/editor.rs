//! The tag editor (CONCEPT.md §10.3): cdport's own by default (a page in
//! the browser, started through cdport, not a command), or Ear Tag, Kid3,
//! Picard or a custom command. Commands use cdport's template format:
//! `{files}` becomes the files, `{dir}` their folder, `@@` marks flatpak's
//! file forwarding.

use std::path::{Path, PathBuf};

pub const EAR_TAG_ID: &str = "app.drey.EarTag";
pub const EAR_TAG_INSTALL_URI: &str = "appstream://app.drey.EarTag";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Editor {
    /// cdport's tag editor (cdport 1.4.0 on), one album folder at a time.
    Cdport,
    EarTag,
    Kid3,
    Picard,
    /// A command line, split on whitespace, with `{files}` / `{dir}`.
    Custom(String),
}

impl Editor {
    /// From the `tag-editor` setting ("auto", from before cdport had its
    /// own editor, and anything unknown: cdport's).
    pub fn from_name(name: &str, custom: &str) -> Self {
        match name {
            "eartag" => Self::EarTag,
            "kid3" => Self::Kid3,
            "picard" => Self::Picard,
            "custom" if !custom.trim().is_empty() => Self::Custom(custom.to_owned()),
            _ => Self::Cdport,
        }
    }

    pub fn name(&self) -> &str {
        match self {
            Self::Cdport => "cdport",
            Self::EarTag => "Ear Tag",
            Self::Kid3 => "Kid3",
            Self::Picard => "MusicBrainz Picard",
            Self::Custom(_) => "custom editor",
        }
    }

    /// Command templates to try in order (native first, then Flatpak),
    /// the same as cdport's defaults. None for cdport's own editor.
    pub fn templates(&self) -> Vec<Vec<String>> {
        let t = |parts: &[&str]| parts.iter().map(|p| (*p).to_owned()).collect::<Vec<_>>();
        match self {
            Self::Cdport => Vec::new(),
            Self::EarTag => vec![t(&[
                "flatpak",
                "run",
                "--file-forwarding",
                EAR_TAG_ID,
                "@@",
                "{files}",
                "@@",
            ])],
            Self::Kid3 => vec![
                t(&["kid3", "{dir}"]),
                t(&[
                    "flatpak",
                    "run",
                    "--file-forwarding",
                    "org.kde.kid3",
                    "@@",
                    "{dir}",
                    "@@",
                ]),
            ],
            Self::Picard => vec![
                t(&["picard", "{files}"]),
                t(&[
                    "flatpak",
                    "run",
                    "--file-forwarding",
                    "org.musicbrainz.Picard",
                    "@@",
                    "{files}",
                    "@@",
                ]),
            ],
            Self::Custom(command) => vec![command.split_whitespace().map(str::to_owned).collect()],
        }
    }

    /// The Flatpak app ID a template runs, for detection (`flatpak info`).
    pub fn flatpak_id(template: &[String]) -> Option<&str> {
        (template.first().map(String::as_str) == Some("flatpak"))
            .then(|| {
                template
                    .iter()
                    .skip(2)
                    .find(|a| !a.starts_with('-'))
                    .map(String::as_str)
            })
            .flatten()
    }
}

/// Fills a template in: `{files}` expands to every file, `{dir}` to their
/// common folder.
pub fn expand(template: &[String], files: &[PathBuf]) -> Vec<String> {
    let dir = common_dir(files);
    let mut args = Vec::new();
    for part in template {
        match part.as_str() {
            "{files}" => args.extend(files.iter().map(|f| f.display().to_string())),
            "{dir}" => args.extend(dir.iter().map(|d| d.display().to_string())),
            _ => args.push(part.clone()),
        }
    }
    args
}

fn common_dir(files: &[PathBuf]) -> Option<PathBuf> {
    let mut dir = files.first()?.parent()?.to_path_buf();
    for file in files {
        while !file.starts_with(&dir) {
            dir = dir.parent().map(Path::to_path_buf)?;
        }
    }
    Some(dir)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ear_tag_command() {
        let files = vec![PathBuf::from("/m/A/01.flac"), PathBuf::from("/m/A/02.flac")];
        let template = &Editor::EarTag.templates()[0];
        assert_eq!(
            expand(template, &files),
            [
                "flatpak",
                "run",
                "--file-forwarding",
                "app.drey.EarTag",
                "@@",
                "/m/A/01.flac",
                "/m/A/02.flac",
                "@@"
            ]
        );
        assert_eq!(Editor::flatpak_id(template), Some(EAR_TAG_ID));
    }

    #[test]
    fn dirs_and_custom_commands() {
        let files = vec![
            PathBuf::from("/m/A/CD1/01.flac"),
            PathBuf::from("/m/A/CD2/01.flac"),
        ];
        assert_eq!(
            expand(&Editor::Kid3.templates()[0], &files),
            ["kid3", "/m/A"]
        );
        let custom = Editor::from_name("custom", "my-editor --open {files}");
        assert_eq!(
            expand(&custom.templates()[0], &files[..1]),
            ["my-editor", "--open", "/m/A/CD1/01.flac"]
        );
        assert_eq!(Editor::from_name("custom", " "), Editor::Cdport);
        assert_eq!(Editor::from_name("auto", ""), Editor::Cdport);
        assert_eq!(Editor::from_name("eartag", ""), Editor::EarTag);
        assert_eq!(Editor::flatpak_id(&custom.templates()[0]), None);
    }
}
