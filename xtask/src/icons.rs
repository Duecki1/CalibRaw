//! Application icon generation for desktop packaging and Android resources.

use crate::process::workspace_root;
use crate::{Result, XtaskError};
use image::{imageops::FilterType, DynamicImage, ImageFormat, RgbaImage};
use std::fs::{self, File};
use std::io::{Cursor, Write};
use std::path::Path;

/// `cargo xtask icons`: renders every packaged icon from `CalibRawIcon.png`.
pub(crate) fn command_icons() -> Result<()> {
    let source = load_icon_source()?;
    let output = workspace_root().join("packaging/icons");
    fs::create_dir_all(&output)?;
    DynamicImage::ImageRgba8(render_icon(&source, 1024))
        .save_with_format(output.join("calibraw-1024.png"), ImageFormat::Png)
        .map_err(|error| XtaskError::new(format!("cannot write calibraw-1024.png: {error}")))?;
    DynamicImage::ImageRgba8(render_icon(&source, 256))
        .save_with_format(output.join("calibraw-256.png"), ImageFormat::Png)
        .map_err(|error| XtaskError::new(format!("cannot write calibraw-256.png: {error}")))?;
    write_ico(&output.join("calibraw.ico"), &source)?;
    write_android_launcher_icons(&source)?;
    Ok(())
}

fn load_icon_source() -> Result<RgbaImage> {
    let path = workspace_root().join("CalibRawIcon.png");
    let image = image::open(&path)
        .map_err(|error| XtaskError::new(format!("cannot open {}: {error}", path.display())))?
        .into_rgba8();
    let (width, height) = image.dimensions();
    if width == 0 || height == 0 || width != height {
        return Err(XtaskError::new(format!(
            "icon source must be a non-empty square PNG: {} is {width}x{height}",
            path.display()
        )));
    }
    Ok(image)
}

fn render_icon(source: &RgbaImage, edge: u32) -> RgbaImage {
    image::imageops::resize(source, edge, edge, FilterType::Lanczos3)
}

fn encode_png(image: RgbaImage) -> Result<Vec<u8>> {
    let mut bytes = Cursor::new(Vec::new());
    DynamicImage::ImageRgba8(image)
        .write_to(&mut bytes, ImageFormat::Png)
        .map_err(|error| XtaskError::new(format!("cannot encode icon PNG: {error}")))?;
    Ok(bytes.into_inner())
}

fn write_ico(path: &Path, source: &RgbaImage) -> Result<()> {
    let sizes = [16_u32, 24, 32, 48, 64, 128, 256];
    let images: Vec<Vec<u8>> = sizes
        .iter()
        .map(|edge| encode_png(render_icon(source, *edge)))
        .collect::<Result<_>>()?;
    let mut file = File::create(path)
        .map_err(|error| XtaskError::new(format!("cannot create {}: {error}", path.display())))?;
    file.write_all(&0_u16.to_le_bytes())?;
    file.write_all(&1_u16.to_le_bytes())?;
    file.write_all(&(sizes.len() as u16).to_le_bytes())?;
    let mut offset = 6_u32 + 16_u32 * sizes.len() as u32;
    for (edge, bytes) in sizes.iter().zip(&images) {
        file.write_all(&[if *edge == 256 { 0 } else { *edge as u8 }])?;
        file.write_all(&[if *edge == 256 { 0 } else { *edge as u8 }])?;
        file.write_all(&[0, 0])?;
        file.write_all(&1_u16.to_le_bytes())?;
        file.write_all(&32_u16.to_le_bytes())?;
        file.write_all(&(bytes.len() as u32).to_le_bytes())?;
        file.write_all(&offset.to_le_bytes())?;
        offset += bytes.len() as u32;
    }
    for bytes in images {
        file.write_all(&bytes)?;
    }
    Ok(())
}

fn write_android_launcher_icons(source: &RgbaImage) -> Result<()> {
    let root = workspace_root();
    let mipmap = root.join("android/app/src/main/res/mipmap-anydpi");
    fs::create_dir_all(&mipmap)?;
    for name in ["ic_launcher.png", "ic_launcher_round.png"] {
        DynamicImage::ImageRgba8(source.clone())
            .save_with_format(mipmap.join(name), ImageFormat::Png)
            .map_err(|error| XtaskError::new(format!("cannot write Android {name}: {error}")))?;
    }

    for obsolete in [
        root.join("android/app/src/main/res/mipmap-anydpi/ic_launcher.xml"),
        root.join("android/app/src/main/res/mipmap-anydpi/ic_launcher_round.xml"),
    ] {
        if obsolete.exists() {
            fs::remove_file(&obsolete).map_err(|error| {
                XtaskError::new(format!("cannot remove {}: {error}", obsolete.display()))
            })?;
        }
    }

    let mut cutout = source.clone();
    for pixel in cutout.pixels_mut() {
        if pixel[0] == 255 && pixel[1] == 255 && pixel[2] == 255 && pixel[3] == 255 {
            pixel[3] = 0;
        }
    }
    let foreground_edge = (source.width() * 66 / 100).max(1);
    let scaled = render_icon(&cutout, foreground_edge);
    let mut foreground = RgbaImage::new(source.width(), source.height());
    let offset = i64::from((source.width() - foreground_edge) / 2);
    image::imageops::overlay(&mut foreground, &scaled, offset, offset);

    let drawable = root.join("android/app/src/main/res/drawable-nodpi");
    fs::create_dir_all(&drawable)?;
    DynamicImage::ImageRgba8(foreground)
        .save_with_format(
            drawable.join("calibraw_icon_foreground.png"),
            ImageFormat::Png,
        )
        .map_err(|error| XtaskError::new(format!("cannot write Android adaptive icon: {error}")))?;
    Ok(())
}
