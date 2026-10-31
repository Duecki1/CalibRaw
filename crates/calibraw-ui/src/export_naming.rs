use crate::i18n::{tr, tr_fmt};
use crate::pipeline::{LoadedRaw, RawDisplayMetadata};
use std::path::Path;
use std::time::SystemTime;

pub(crate) const DEFAULT_EXPORT_NAME_TEMPLATE: &str = "{OriginalName}-CalibRaw";
pub(crate) const MAX_EXPORT_NAME_TEMPLATE_CHARS: usize = 200;

#[derive(Clone, Debug)]
pub(crate) struct ExportNameContext {
    original_name: String,
    current_date: String,
    edited_date: String,
    iso_speed: f32,
    shutter_seconds: f32,
    focal_length: f32,
}

impl ExportNameContext {
    pub(crate) fn from_raw(
        original_name: impl Into<String>,
        raw: &LoadedRaw,
        edited_at: Option<SystemTime>,
    ) -> Self {
        Self::new(
            original_name,
            raw.capture_metadata.iso_speed,
            raw.capture_metadata.shutter_seconds,
            raw.focal_length,
            edited_at,
        )
    }

    pub(crate) fn from_display_metadata(
        original_name: impl Into<String>,
        metadata: Option<RawDisplayMetadata>,
        edited_at: Option<SystemTime>,
    ) -> Self {
        let metadata = metadata.unwrap_or_default();
        Self::new(
            original_name,
            metadata.iso_speed,
            metadata.shutter_seconds,
            metadata.focal_length,
            edited_at,
        )
    }

    fn new(
        original_name: impl Into<String>,
        iso_speed: f32,
        shutter_seconds: f32,
        focal_length: f32,
        edited_at: Option<SystemTime>,
    ) -> Self {
        let current_date = local_date(SystemTime::now()).unwrap_or_else(|| "unknown".to_owned());
        let edited_date = edited_at
            .and_then(local_date)
            .unwrap_or_else(|| current_date.clone());
        Self {
            original_name: original_name.into(),
            current_date,
            edited_date,
            iso_speed,
            shutter_seconds,
            focal_length,
        }
    }
}

pub(crate) fn sanitize_template_setting(template: &str) -> String {
    let sanitized = template
        .chars()
        .take(MAX_EXPORT_NAME_TEMPLATE_CHARS)
        .map(|character| match character {
            '\r' | '\n' | '\t' => ' ',
            other => other,
        })
        .collect::<String>();
    let sanitized = sanitized.trim();
    if sanitized.is_empty() {
        DEFAULT_EXPORT_NAME_TEMPLATE.to_owned()
    } else {
        sanitized.to_owned()
    }
}

pub(crate) fn render_export_stem(
    template: &str,
    context: &ExportNameContext,
) -> Result<String, String> {
    let template = template.trim();
    if template.is_empty() {
        return Err(tr("The export name template cannot be empty.").to_owned());
    }

    let mut rendered = String::with_capacity(template.len() + context.original_name.len());
    let mut remaining = template;
    while let Some(open) = remaining.find('{') {
        if remaining[..open].contains('}') {
            return Err(tr("The export name template contains an unmatched `}`.").to_owned());
        }
        rendered.push_str(&remaining[..open]);
        let token_start = open + 1;
        let Some(close_offset) = remaining[token_start..].find('}') else {
            return Err(tr("The export name template contains an unmatched `{`.").to_owned());
        };
        let close = token_start + close_offset;
        let token = &remaining[token_start..close];
        let value = token_value(token, context).ok_or_else(|| {
            tr_fmt(
                "Unknown export name placeholder `{placeholder}`. Use one of the placeholders shown below.",
                &[("placeholder", &format!("{{{token}}}"))],
            )
        })?;
        rendered.push_str(&value);
        remaining = &remaining[close + 1..];
    }
    if remaining.contains('}') {
        return Err(tr("The export name template contains an unmatched `}`.").to_owned());
    }
    rendered.push_str(remaining);

    Ok(sanitize_file_stem(&rendered))
}

pub(crate) fn render_export_stem_or_default(template: &str, context: &ExportNameContext) -> String {
    render_export_stem(template, context).unwrap_or_else(|_| {
        render_export_stem(DEFAULT_EXPORT_NAME_TEMPLATE, context)
            .unwrap_or_else(|_| "calibraw-export".to_owned())
    })
}

/// The first free export file name in `directory`, for a save dialog that
/// opens there. Without a directory the dialog's folder is unknown, so the
/// plain name is returned.
pub(crate) fn free_export_file_name(
    directory: Option<&Path>,
    stem: &str,
    extension: &str,
) -> String {
    let directory = directory.filter(|directory| !directory.as_os_str().is_empty());
    calibraw_core::file_ops::first_free_file_name(stem, extension, |name| {
        directory.is_some_and(|directory| directory.join(name).exists())
    })
}

/// A stem for which `{stem}-1.{extension}` up to `{stem}-{slides}.{extension}` are all free in
/// `directory`: `stem` itself, or the first free `{stem}-2`, `{stem}-3`, ….
pub(crate) fn free_carousel_stem(
    directory: &Path,
    stem: &str,
    extension: &str,
    slides: usize,
) -> String {
    let is_free = |candidate: &str| {
        (1..=slides).all(|slide| {
            !directory
                .join(format!("{candidate}-{slide}.{extension}"))
                .exists()
        })
    };
    let mut candidate = stem.to_owned();
    let mut index = 2usize;
    while !is_free(&candidate) {
        candidate = format!("{stem}-{index}");
        index += 1;
    }
    candidate
}

pub(crate) fn edited_time_for_path(path: &Path) -> Option<SystemTime> {
    std::fs::metadata(crate::sidecar::sidecar_path_for_raw(path))
        .and_then(|metadata| metadata.modified())
        .or_else(|_| std::fs::metadata(path).and_then(|metadata| metadata.modified()))
        .ok()
}

fn token_value(token: &str, context: &ExportNameContext) -> Option<String> {
    match token.trim().to_ascii_lowercase().as_str() {
        "originalname" => Some(context.original_name.clone()),
        "currentdate" => Some(context.current_date.clone()),
        "editeddate" => Some(context.edited_date.clone()),
        "iso" => Some(format_iso(context.iso_speed)),
        "shutterspeed" => Some(format_shutter_speed(context.shutter_seconds)),
        // Keep the common transposition as an alias for templates typed from memory.
        "focallength" | "focallenght" => Some(format_focal_length(context.focal_length)),
        _ => None,
    }
}

fn format_iso(value: f32) -> String {
    if value.is_finite() && value > 0.0 {
        format!("{value:.0}")
    } else {
        "unknown".to_owned()
    }
}

fn format_shutter_speed(seconds: f32) -> String {
    if !seconds.is_finite() || seconds <= 0.0 {
        return "unknown".to_owned();
    }
    if seconds < 1.0 {
        let reciprocal = (1.0 / seconds).round().max(1.0);
        if ((1.0 / reciprocal) - seconds).abs() <= seconds * 0.02 {
            return format!("1-{reciprocal:.0}s");
        }
    }
    format!("{}s", concise_decimal(seconds))
}

fn format_focal_length(value: f32) -> String {
    if value.is_finite() && value > 0.0 {
        format!("{}mm", concise_decimal(value))
    } else {
        "unknown".to_owned()
    }
}

fn concise_decimal(value: f32) -> String {
    let value = format!("{value:.2}");
    value.trim_end_matches('0').trim_end_matches('.').to_owned()
}

fn local_date(time: SystemTime) -> Option<String> {
    jiff::Zoned::try_from(time)
        .ok()
        .map(|zoned| zoned.strftime("%Y-%m-%d").to_string())
}

fn sanitize_file_stem(value: &str) -> String {
    let mut stem = value
        .chars()
        .map(|character| {
            if character.is_control()
                || matches!(
                    character,
                    '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|'
                )
            {
                '_'
            } else {
                character
            }
        })
        .take(180)
        .collect::<String>();
    stem = stem.trim().trim_matches(['.', ' ']).to_owned();
    if stem.is_empty() {
        return "calibraw-export".to_owned();
    }
    if is_windows_reserved_name(&stem) {
        stem.push('_');
    }
    stem
}

fn is_windows_reserved_name(stem: &str) -> bool {
    let base = stem.split('.').next().unwrap_or(stem);
    matches!(
        base.to_ascii_uppercase().as_str(),
        "CON"
            | "PRN"
            | "AUX"
            | "NUL"
            | "COM1"
            | "COM2"
            | "COM3"
            | "COM4"
            | "COM5"
            | "COM6"
            | "COM7"
            | "COM8"
            | "COM9"
            | "LPT1"
            | "LPT2"
            | "LPT3"
            | "LPT4"
            | "LPT5"
            | "LPT6"
            | "LPT7"
            | "LPT8"
            | "LPT9"
    )
}
