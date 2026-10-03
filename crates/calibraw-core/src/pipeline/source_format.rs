//! Identifies display-referred photos (JPEG, PNG, HEIC/HEIF) so they can be
//! routed to the raster pipeline instead of the sensor decoders.

use anyhow::{Context, Result};
use std::fs::File;
use std::io::Read;
use std::path::Path;

/// A rendered, display-referred image format that CalibRaw can edit.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum RenderedImageFormat {
    Jpeg,
    Png,
    Heif,
}

/// Rendered-image extensions accepted next to [`super::SUPPORTED_RAW_EXTENSIONS`].
/// `hif` is the extension Fujifilm and Canon cameras use for HEIF captures.
pub const SUPPORTED_RENDERED_EXTENSIONS: &[&str] =
    &["jpg", "jpeg", "jpe", "png", "heic", "heif", "hif"];

/// HEVC-coded HEIF brands. AVIF and other MIAF images share the container but
/// not the codec, so they are not claimed here.
const HEVC_HEIF_BRANDS: &[&[u8; 4]] = &[b"heic", b"heix", b"heim", b"heis", b"hevc", b"hevx"];
const SNIFF_BYTES: usize = 64;

impl RenderedImageFormat {
    /// Short format name for badges and metadata panels.
    pub const fn label(self) -> &'static str {
        match self {
            Self::Jpeg => "JPEG",
            Self::Png => "PNG",
            Self::Heif => "HEIC",
        }
    }

    pub fn from_extension(extension: &str) -> Option<Self> {
        let extension = extension.to_ascii_lowercase();
        match extension.as_str() {
            "jpg" | "jpeg" | "jpe" => Some(Self::Jpeg),
            "png" => Some(Self::Png),
            "heic" | "heif" | "hif" => Some(Self::Heif),
            _ => None,
        }
    }

    /// Classifies a file by its name only. Use [`Self::detect`] for decoding.
    pub fn from_path(path: &Path) -> Option<Self> {
        path.extension()
            .and_then(|extension| extension.to_str())
            .and_then(Self::from_extension)
    }

    /// Identifies a format from the first bytes of a file.
    pub fn sniff(header: &[u8]) -> Option<Self> {
        if header.starts_with(&[0xFF, 0xD8, 0xFF]) {
            return Some(Self::Jpeg);
        }
        if header.starts_with(b"\x89PNG\r\n\x1a\n") {
            return Some(Self::Png);
        }
        is_hevc_heif(header).then_some(Self::Heif)
    }

    /// Decides whether `path` should be decoded as a rendered image.
    ///
    /// RAW extensions always keep their sensor decoders. Rendered extensions
    /// trust the file content over a mislabelled extension, and paths without
    /// a known extension (Android opens documents as `/proc/self/fd/<n>`) are
    /// identified from their content alone.
    pub fn detect(path: &Path) -> Result<Option<Self>> {
        if super::is_supported_raw_path(path) {
            return Ok(None);
        }
        let header = read_header(path)?;
        Ok(Self::sniff(&header).or_else(|| Self::from_path(path)))
    }
}

/// Whether CalibRaw can open `path`, judged by its extension.
pub fn is_supported_image_path(path: &Path) -> bool {
    super::is_supported_raw_path(path) || RenderedImageFormat::from_path(path).is_some()
}

/// Whether `path` names a camera RAW, judged by its extension. TIFFs are
/// excluded because they are usually rendered images, not sensor captures.
pub fn is_camera_raw_path(path: &Path) -> bool {
    super::is_supported_raw_path(path) && !super::tiff_loader::is_tiff_path(path)
}

/// Every extension CalibRaw opens: RAW and TIFF first, then rendered images.
pub fn supported_image_extensions() -> impl Iterator<Item = &'static str> {
    super::SUPPORTED_RAW_EXTENSIONS
        .iter()
        .chain(SUPPORTED_RENDERED_EXTENSIONS)
        .copied()
}

fn read_header(path: &Path) -> Result<Vec<u8>> {
    let mut header = Vec::with_capacity(SNIFF_BYTES);
    File::open(path)
        .with_context(|| format!("open {}", path.display()))?
        .take(SNIFF_BYTES as u64)
        .read_to_end(&mut header)
        .with_context(|| format!("read the header of {}", path.display()))?;
    Ok(header)
}

fn is_hevc_heif(header: &[u8]) -> bool {
    if header.len() < 16 || &header[4..8] != b"ftyp" {
        return false;
    }
    let box_size = u32::from_be_bytes([header[0], header[1], header[2], header[3]]) as usize;
    let end = box_size.clamp(16, header.len());
    // Major brand at 8..12, minor version at 12..16, compatible brands after.
    std::iter::once(&header[8..12])
        .chain(header[16..end].chunks_exact(4))
        .any(|brand| {
            HEVC_HEIF_BRANDS
                .iter()
                .any(|known| brand == known.as_slice())
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ftyp(major: &[u8; 4], compatible: &[&[u8; 4]]) -> Vec<u8> {
        let size = 16 + 4 * compatible.len();
        let mut bytes = (size as u32).to_be_bytes().to_vec();
        bytes.extend_from_slice(b"ftyp");
        bytes.extend_from_slice(major);
        bytes.extend_from_slice(&[0; 4]);
        for brand in compatible {
            bytes.extend_from_slice(*brand);
        }
        bytes
    }

    #[test]
    fn extensions_classify_case_insensitively() {
        for (name, expected) in [
            ("a.JPG", Some(RenderedImageFormat::Jpeg)),
            ("a.jpeg", Some(RenderedImageFormat::Jpeg)),
            ("a.Png", Some(RenderedImageFormat::Png)),
            ("a.HEIC", Some(RenderedImageFormat::Heif)),
            ("a.hif", Some(RenderedImageFormat::Heif)),
            ("a.cr3", None),
            ("a.tif", None),
            ("a", None),
        ] {
            assert_eq!(
                RenderedImageFormat::from_path(Path::new(name)),
                expected,
                "{name}"
            );
        }
    }

    #[test]
    fn supported_image_paths_include_raw_and_rendered_formats() {
        for name in ["a.nef", "a.DNG", "a.tiff", "a.jpg", "a.png", "a.heif"] {
            assert!(is_supported_image_path(Path::new(name)), "{name}");
        }
        for name in ["a.txt", "a.calibraw", "a.mp4", "a"] {
            assert!(!is_supported_image_path(Path::new(name)), "{name}");
        }
        assert!(is_camera_raw_path(Path::new("a.NEF")));
        assert!(!is_camera_raw_path(Path::new("a.tif")));
        assert!(!is_camera_raw_path(Path::new("a.jpg")));
        assert!(supported_image_extensions().any(|extension| extension == "heic"));
        assert!(supported_image_extensions().any(|extension| extension == "arw"));
    }

    #[test]
    fn sniffing_recognizes_magic_numbers() {
        assert_eq!(
            RenderedImageFormat::sniff(&[0xFF, 0xD8, 0xFF, 0xE1, 0, 0]),
            Some(RenderedImageFormat::Jpeg)
        );
        assert_eq!(
            RenderedImageFormat::sniff(b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR"),
            Some(RenderedImageFormat::Png)
        );
        assert_eq!(
            RenderedImageFormat::sniff(&ftyp(b"heic", &[b"mif1", b"heic"])),
            Some(RenderedImageFormat::Heif)
        );
        assert_eq!(
            RenderedImageFormat::sniff(&ftyp(b"mif1", &[b"mif1", b"heic"])),
            Some(RenderedImageFormat::Heif)
        );
    }

    #[test]
    fn sniffing_leaves_other_isobmff_and_tiff_files_alone() {
        // Canon CR3 and AVIF share the ISOBMFF container with HEIF.
        assert_eq!(
            RenderedImageFormat::sniff(&ftyp(b"crx ", &[b"crx ", b"isom"])),
            None
        );
        assert_eq!(
            RenderedImageFormat::sniff(&ftyp(b"avif", &[b"mif1", b"miaf"])),
            None
        );
        assert_eq!(RenderedImageFormat::sniff(b"II*\0\x08\0\0\0"), None);
        assert_eq!(RenderedImageFormat::sniff(b""), None);
    }

    #[test]
    fn detect_trusts_content_and_keeps_raw_extensions_on_sensor_decoders() {
        let directory = tempfile::tempdir().unwrap();
        let jpeg_bytes = [0xFF, 0xD8, 0xFF, 0xE0, 0, 0, 0, 0];

        let extensionless = directory.path().join("17");
        std::fs::write(&extensionless, jpeg_bytes).unwrap();
        assert_eq!(
            RenderedImageFormat::detect(&extensionless).unwrap(),
            Some(RenderedImageFormat::Jpeg)
        );

        let mislabelled = directory.path().join("photo.heic");
        std::fs::write(&mislabelled, jpeg_bytes).unwrap();
        assert_eq!(
            RenderedImageFormat::detect(&mislabelled).unwrap(),
            Some(RenderedImageFormat::Jpeg)
        );

        let raw = directory.path().join("photo.dng");
        std::fs::write(&raw, jpeg_bytes).unwrap();
        assert_eq!(RenderedImageFormat::detect(&raw).unwrap(), None);

        let unknown = directory.path().join("raw-data");
        std::fs::write(&unknown, b"II*\0\x08\0\0\0").unwrap();
        assert_eq!(RenderedImageFormat::detect(&unknown).unwrap(), None);
    }
}
