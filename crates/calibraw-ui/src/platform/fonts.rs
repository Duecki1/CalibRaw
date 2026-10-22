//! A system font for Chinese text, which egui's built-in fonts lack; the
//! theme adds it after them (`ui::theme::install`). Nothing is bundled: CJK
//! fonts are about 20 MB, and every OS that shows Chinese file names has one.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// A face of a font file, mapped for the rest of the process.
#[derive(Clone, Copy)]
pub(crate) struct SystemFont {
    pub(crate) bytes: &'static [u8],
    /// Face within a collection (`.ttc`); 0 for single fonts.
    pub(crate) index: u32,
}

/// The system's Chinese font, or `None` when none is installed. Looked up
/// and mapped once.
pub(crate) fn chinese_font() -> Option<SystemFont> {
    static FONT: OnceLock<Option<SystemFont>> = OnceLock::new();
    *FONT.get_or_init(|| {
        let (path, index) = chinese_font_file()?;
        match map_font(&path) {
            Ok(bytes) => Some(SystemFont { bytes, index }),
            Err(error) => {
                log::warn!("could not map Chinese font {}: {error}", path.display());
                None
            }
        }
    })
}

/// The Chinese font each OS ships, with the face to use; fontconfig's choice
/// elsewhere.
fn chinese_font_file() -> Option<(PathBuf, u32)> {
    let (path, index) = if cfg!(target_os = "windows") {
        let root = std::env::var_os("SystemRoot")?;
        (Path::new(&root).join(r"Fonts\msyh.ttc"), 0)
    } else if cfg!(target_os = "macos") {
        (
            PathBuf::from("/System/Library/Fonts/Hiragino Sans GB.ttc"),
            0,
        )
    } else if cfg!(target_os = "android") {
        // Face 2 is Simplified Chinese, as Android's fonts.xml maps zh-Hans.
        (PathBuf::from("/system/fonts/NotoSansCJK-Regular.ttc"), 2)
    } else {
        return fontconfig_chinese_font();
    };
    path.is_file().then_some((path, index))
}

/// The font fontconfig prefers for Simplified Chinese, honouring the user's
/// configuration, or `None` when `fc-match` is missing or only offers a font
/// without Chinese.
fn fontconfig_chinese_font() -> Option<(PathBuf, u32)> {
    let output = std::process::Command::new("fc-match")
        .args([
            "--format=%{file}\n%{index}\n%{lang}\n",
            "sans-serif:lang=zh-cn",
        ])
        .output()
        .ok()
        .filter(|output| output.status.success())?;
    parse_fc_match(&String::from_utf8_lossy(&output.stdout))
}

/// `fc-match` prints the best match even when no font covers the language,
/// so the match counts only when its language set lists `zh-cn`.
fn parse_fc_match(output: &str) -> Option<(PathBuf, u32)> {
    let mut lines = output.lines();
    let path = PathBuf::from(lines.next()?);
    let index = lines.next()?.parse().ok()?;
    let covers_chinese = lines.next()?.split('|').any(|lang| lang == "zh-cn");
    covers_chinese.then_some((path, index))
}

/// Maps `path` read-only and keeps the mapping until the process exits, as
/// egui holds font data for its lifetime. Mapping instead of reading keeps a
/// 20 MB collection out of the heap; its pages are shared with every other
/// program using the font.
fn map_font(path: &Path) -> std::io::Result<&'static [u8]> {
    let file = std::fs::File::open(path)?;
    // SAFETY: the mapping stays valid only while nobody truncates or rewrites
    // the file in place. System font files are replaced by package managers
    // through a rename, which leaves this mapping on the old, unchanged file.
    let mapping = unsafe { memmap2::Mmap::map(&file) }?;
    Ok(Box::leak(Box::new(mapping)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fc_match_counts_only_fonts_covering_chinese() {
        let cjk = "/usr/share/fonts/noto-cjk/NotoSansCJK-Regular.ttc\n1\naa|ja|ko|zh-cn|zh-tw\n";
        assert_eq!(
            parse_fc_match(cjk),
            Some((
                PathBuf::from("/usr/share/fonts/noto-cjk/NotoSansCJK-Regular.ttc"),
                1
            ))
        );
        let latin = "/usr/share/fonts/noto/NotoSans-Regular.ttf\n0\naa|de|en|zh-cnx\n";
        assert_eq!(parse_fc_match(latin), None);
        assert_eq!(parse_fc_match(""), None);
        assert_eq!(parse_fc_match("/font.ttf\nx\nzh-cn\n"), None);
    }
}
