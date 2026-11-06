//! Cover thumbnails in `~/.cache/<app id>/covers/` (CONCEPT.md §4.3).
//!
//! Covers come from files we don't control, so they're decoded by glycin's
//! sandboxed loaders. Only the decoded pixels are resized and written back
//! as JPEG thumbnails, which the UI then loads.

use std::fs;
use std::io::{self, BufWriter};
use std::path::{Path, PathBuf};

use glycin::{Loader, MainContextSelector, MemoryFormatSelection};
use image::imageops::FilterType;
use image::{DynamicImage, RgbaImage};

/// Thumbnail sizes (longest side in pixels).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CoverSize {
    /// Album grid tiles and the mini player.
    Grid,
    /// The album side panel.
    Panel,
    /// The full player view and MPRIS.
    Player,
}

impl CoverSize {
    pub const ALL: [Self; 3] = [Self::Grid, Self::Panel, Self::Player];

    pub fn pixels(self) -> u32 {
        match self {
            Self::Grid => 360,
            Self::Panel => 720,
            Self::Player => 1200,
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum CoverError {
    #[error("cannot decode cover: {0}")]
    Decode(String),
    #[error("cannot write cover thumbnail: {0}")]
    Io(#[from] io::Error),
    #[error("cannot encode cover thumbnail: {0}")]
    Encode(#[from] image::ImageError),
}

/// File names of album-folder covers, in priority order (without extension).
const FOLDER_COVER_NAMES: &[&str] = &["cover", "folder", "front"];
const FOLDER_COVER_EXTENSIONS: &[&str] = &["jpg", "jpeg", "png", "webp"];

#[derive(Debug, Clone)]
pub struct CoverCache {
    dir: PathBuf,
}

impl CoverCache {
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    pub fn path(&self, key: &str, size: CoverSize) -> PathBuf {
        self.dir.join(format!("{key}-{}.jpg", size.pixels()))
    }

    pub fn has(&self, key: &str) -> bool {
        CoverSize::ALL
            .iter()
            .all(|&size| self.path(key, size).is_file())
    }

    /// Decodes `data` in glycin's sandbox and writes all thumbnail sizes.
    pub fn generate(&self, key: &str, data: Vec<u8>) -> Result<(), CoverError> {
        let image = decode(data)?;
        fs::create_dir_all(&self.dir)?;
        for size in CoverSize::ALL {
            let pixels = size.pixels();
            let scaled = if image.width() > pixels || image.height() > pixels {
                image.resize(pixels, pixels, FilterType::CatmullRom)
            } else {
                image.clone()
            };
            let path = self.path(key, size);
            let partial = path.with_extension("jpg.part");
            {
                let file = BufWriter::new(fs::File::create(&partial)?);
                let encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(file, 90);
                DynamicImage::ImageRgb8(scaled.to_rgb8()).write_with_encoder(encoder)?;
            }
            fs::rename(&partial, &path)?;
        }
        Ok(())
    }
}

fn decode(data: Vec<u8>) -> Result<DynamicImage, CoverError> {
    let decode_error = |err: glycin::Error| CoverError::Decode(err.to_string());
    futures_lite::future::block_on(async move {
        let mut loader = Loader::new_vec(data);
        loader
            .accepted_memory_formats(MemoryFormatSelection::R8g8b8a8)
            .main_context_selector(MainContextSelector::Managed);
        let mut image = loader.load().await.map_err(decode_error)?;
        let frame = image.next_frame().await.map_err(decode_error)?;

        let (width, height) = (frame.width(), frame.height());
        let stride = frame.stride() as usize;
        let row = width as usize * 4;
        let buf = frame.buf_slice();
        let mut pixels = Vec::with_capacity(row * height as usize);
        for y in 0..height as usize {
            let start = y * stride;
            pixels.extend_from_slice(buf.get(start..start + row).ok_or_else(|| {
                CoverError::Decode("frame buffer is smaller than its size".into())
            })?);
        }
        RgbaImage::from_raw(width, height, pixels)
            .map(DynamicImage::ImageRgba8)
            .ok_or_else(|| CoverError::Decode("frame buffer is smaller than its size".into()))
    })
}

/// A cache key for cover bytes: `<prefix><FNV-1a 64 hash>`.
pub fn cover_key(prefix: char, data: &[u8]) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in data {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0100_0000_01b3);
    }
    format!("{prefix}{hash:016x}")
}

/// `cover.*`, `folder.*` or `front.*` in a folder (case-insensitive, no symlinks).
pub fn find_folder_cover(folder: &Path) -> Option<PathBuf> {
    let entries = fs::read_dir(folder).ok()?;
    let mut best: Option<(usize, PathBuf)> = None;
    for entry in entries.flatten() {
        if !entry.file_type().is_ok_and(|t| t.is_file()) {
            continue;
        }
        let path = entry.path();
        let (Some(stem), Some(ext)) = (
            path.file_stem().and_then(|s| s.to_str()),
            path.extension().and_then(|s| s.to_str()),
        ) else {
            continue;
        };
        let (stem, ext) = (stem.to_lowercase(), ext.to_lowercase());
        if !FOLDER_COVER_EXTENSIONS.contains(&ext.as_str()) {
            continue;
        }
        if let Some(rank) = FOLDER_COVER_NAMES.iter().position(|n| *n == stem)
            && best.as_ref().is_none_or(|(r, _)| rank < *r)
        {
            best = Some((rank, path));
        }
    }
    best.map(|(_, path)| path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_are_stable() {
        assert_eq!(cover_key('e', b""), "ecbf29ce484222325");
        assert_ne!(cover_key('e', b"a"), cover_key('e', b"b"));
    }

    #[test]
    fn folder_cover_priority() {
        let dir = tempfile::TempDir::new().unwrap();
        assert_eq!(find_folder_cover(dir.path()), None);
        fs::write(dir.path().join("Front.PNG"), b"").unwrap();
        fs::write(dir.path().join("notes.txt"), b"").unwrap();
        assert_eq!(
            find_folder_cover(dir.path()),
            Some(dir.path().join("Front.PNG"))
        );
        fs::write(dir.path().join("cover.jpg"), b"").unwrap();
        assert_eq!(
            find_folder_cover(dir.path()),
            Some(dir.path().join("cover.jpg"))
        );
    }
}
