//! A system font for Chinese text, which egui's built-in fonts lack; the
//! theme adds it after them (`ui::theme::install`). Nothing is bundled: CJK
//! fonts are about 20 MB, and every OS that shows Chinese file names has one.
//!
//! Finding and loading the font is best effort. A missing, unreadable or
//! malformed font is skipped, and the app starts with egui's fonts alone, so
//! Chinese text falls back to the replacement glyph instead of crashing.
//! Candidates are parsed with `skrifa` before they reach `egui::Context`,
//! which panics on a font file or face index it cannot parse.

use std::io;
use std::path::PathBuf;
use std::sync::OnceLock;

use skrifa::MetadataProvider as _;

/// Largest file considered. CJK collections are 20-40 MB; anything much
/// larger is more likely a broken file than a system font.
const MAX_FONT_FILE_BYTES: u64 = 128 * 1024 * 1024;

/// Faces tried in a collection. Noto CJK has ten; vendors may add a few more.
const MAX_FACES: u32 = 32;

/// A face of a font file, kept in memory for the rest of the process.
#[derive(Clone, Copy)]
pub(crate) struct SystemFont {
    pub(crate) bytes: &'static [u8],
    /// Face within a collection (`.ttc`); 0 for single fonts.
    pub(crate) index: u32,
}

struct FontCandidate {
    path: PathBuf,
    /// The face the OS or fontconfig recommends, tried before scanning.
    index: Option<u32>,
}

impl FontCandidate {
    fn new(path: impl Into<PathBuf>) -> Self {
        Self {
            path: path.into(),
            index: None,
        }
    }

    #[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "ios")))]
    fn with_index(path: impl Into<PathBuf>, index: u32) -> Self {
        Self {
            path: path.into(),
            index: Some(index),
        }
    }
}

/// The system's Chinese font, or `None` when none is installed or loadable.
/// Looked up and read once.
pub(crate) fn chinese_font() -> Option<SystemFont> {
    static FONT: OnceLock<Option<SystemFont>> = OnceLock::new();
    *FONT.get_or_init(|| {
        for candidate in candidates() {
            match load_chinese_face(&candidate) {
                Ok(Some(font)) => return Some(font),
                Ok(None) => log::debug!(
                    "skipping font {}: no face covers Chinese",
                    candidate.path.display()
                ),
                Err(error) if error.kind() == io::ErrorKind::NotFound => {
                    log::debug!("no Chinese font at {}", candidate.path.display())
                }
                Err(error) => log::warn!(
                    "could not load Chinese font {}: {error}",
                    candidate.path.display()
                ),
            }
        }
        log::info!("no system Chinese font found; Chinese text will use replacement glyphs");
        None
    })
}

/// The fonts each OS ships, in preference order. Fontconfig's choice comes
/// first on desktop Linux; the other systems name their Chinese font.
fn candidates() -> Vec<FontCandidate> {
    let mut candidates = Vec::new();

    #[cfg(target_os = "windows")]
    if let Some(root) = std::env::var_os("SystemRoot") {
        let fonts = PathBuf::from(root).join("Fonts");
        for name in [
            "msyh.ttc",
            "msyh.ttf",
            "simsun.ttc",
            "simhei.ttf",
            "Deng.ttf",
        ] {
            candidates.push(FontCandidate::new(fonts.join(name)));
        }
    }

    #[cfg(target_os = "macos")]
    for path in [
        "/System/Library/Fonts/PingFang.ttc",
        "/System/Library/Fonts/Supplemental/PingFang.ttc",
        "/System/Library/Fonts/Hiragino Sans GB.ttc",
        "/System/Library/Fonts/STHeiti Light.ttc",
        "/System/Library/Fonts/Supplemental/Songti.ttc",
    ] {
        candidates.push(FontCandidate::new(path));
    }

    #[cfg(target_os = "ios")]
    for path in [
        "/System/Library/Fonts/LanguageSupport/PingFang.ttc",
        "/System/Library/Fonts/PingFang.ttc",
        "/System/Library/Fonts/LanguageSupport/Hiragino Sans GB.ttc",
    ] {
        candidates.push(FontCandidate::new(path));
    }

    #[cfg(target_os = "android")]
    {
        // Face 2 is Simplified Chinese, as Android's fonts.xml maps zh-Hans.
        // Vendors sometimes ship a different collection; scanning the other
        // faces below recovers from that.
        candidates.push(FontCandidate::with_index(
            "/system/fonts/NotoSansCJK-Regular.ttc",
            2,
        ));
        for path in [
            "/system/fonts/NotoSansCJKsc-Regular.otf",
            "/system/fonts/NotoSansSC-Regular.otf",
            "/system/fonts/NotoSansHans-Regular.otf",
            "/system/fonts/DroidSansFallback.ttf",
            "/system/fonts/DroidSansChinese.ttf",
            "/product/fonts/NotoSansCJK-Regular.ttc",
        ] {
            candidates.push(FontCandidate::new(path));
        }
    }

    #[cfg(not(any(
        target_os = "windows",
        target_os = "macos",
        target_os = "ios",
        target_os = "android"
    )))]
    {
        if let Some(candidate) = fontconfig_candidate() {
            candidates.push(candidate);
        }
        // Fallbacks for systems without fontconfig's `fc-match`; the files
        // are present on most distributions that package a Chinese font.
        // Noto's collection has Simplified Chinese at face 2, like Android's.
        for path in [
            "/usr/share/fonts/noto-cjk/NotoSansCJK-Regular.ttc",
            "/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc",
            "/usr/share/fonts/truetype/noto/NotoSansCJK-Regular.ttc",
        ] {
            candidates.push(FontCandidate::with_index(path, 2));
        }
        for path in [
            "/usr/share/fonts/opentype/noto/NotoSansCJKsc-Regular.otf",
            "/usr/share/fonts/truetype/wqy/wqy-zenhei.ttc",
            "/usr/share/fonts/truetype/droid/DroidSansFallbackFull.ttf",
        ] {
            candidates.push(FontCandidate::new(path));
        }
    }

    candidates
}

/// The font fontconfig prefers for Simplified Chinese, honouring the user's
/// configuration, or `None` when `fc-match` is missing or prints nothing.
#[cfg(not(any(
    target_os = "windows",
    target_os = "macos",
    target_os = "ios",
    target_os = "android"
)))]
fn fontconfig_candidate() -> Option<FontCandidate> {
    let output = std::process::Command::new("fc-match")
        .args(["--format=%{file}\n%{index}\n", "sans-serif:lang=zh-cn"])
        .output()
        .ok()
        .filter(|output| output.status.success())?;
    let (path, index) = parse_fc_match(&String::from_utf8_lossy(&output.stdout))?;
    Some(FontCandidate::with_index(path, index))
}

#[cfg(not(any(
    target_os = "windows",
    target_os = "macos",
    target_os = "ios",
    target_os = "android"
)))]
fn parse_fc_match(output: &str) -> Option<(PathBuf, u32)> {
    let mut lines = output.lines();
    let path = PathBuf::from(lines.next()?);
    let fontconfig_index: u32 = lines.next()?.parse().ok()?;
    // Fontconfig reports FreeType's packed index: the low 16 bits are the
    // face in a collection, bits 16-30 select a named instance of a variable
    // font. egui only accepts the face index, and the packed value is out of
    // range for `skrifa`/epaint (`InvalidCollectionIndex`).
    let face = fontconfig_index & 0xffff;
    (!path.as_os_str().is_empty()).then_some((path, face))
}

/// Reads `candidate` and returns its Chinese face, or `None` when the file
/// exists but contains no such face. Missing files are returned as errors and
/// skipped by the caller.
fn load_chinese_face(candidate: &FontCandidate) -> io::Result<Option<SystemFont>> {
    let metadata = std::fs::metadata(&candidate.path)?;
    if !metadata.is_file() {
        return Ok(None);
    }
    if metadata.len() > MAX_FONT_FILE_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("font file is larger than {MAX_FONT_FILE_BYTES} bytes"),
        ));
    }

    let bytes = std::fs::read(&candidate.path)?;
    if bytes.len() as u64 > MAX_FONT_FILE_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("font file is larger than {MAX_FONT_FILE_BYTES} bytes"),
        ));
    }
    let Some(index) = chinese_face(&bytes, candidate.index) else {
        return Ok(None);
    };

    // The font lives as long as the process, like the egui context that
    // borrows it. Leaking the one validated file keeps `SystemFont` a plain
    // `Copy` handle and avoids copying 20 MB into `FontData`.
    let bytes = Box::leak(bytes.into_boxed_slice());
    Ok(Some(SystemFont { bytes, index }))
}

/// The preferred face when it covers Chinese, otherwise the first face that
/// does. Trying the preferred face first keeps the language variant the OS
/// chose (Simplified rather than Japanese, for example).
fn chinese_face(bytes: &[u8], preferred: Option<u32>) -> Option<u32> {
    if preferred.is_some_and(|index| covers_chinese(bytes, index)) {
        return preferred;
    }
    (0..MAX_FACES).find(|&index| Some(index) != preferred && covers_chinese(bytes, index))
}

/// Whether `bytes` at `index` is a font whose character map has common
/// Chinese characters. The `charmap` check also rejects font formats egui
/// cannot draw.
fn covers_chinese(bytes: &[u8], index: u32) -> bool {
    let Ok(font) = skrifa::FontRef::from_index(bytes, index) else {
        return false;
    };
    let charmap = font.charmap();
    "中文照片".chars().all(|ch| charmap.map(ch).is_some())
}
