//! Generated mask images stored as deduplicated PNG assets, and the scene
//! depth beside them.
//!
//! Depth Range components store no image: they select on the scene depth's
//! selection map, restored from the scene depth.

use super::*;

pub(super) fn generated_mask(geometry: &MaskGeometry) -> Option<&Option<MaskImage>> {
    match geometry {
        MaskGeometry::Ai { mask, .. }
        | MaskGeometry::Object { mask, .. }
        | MaskGeometry::Person { mask, .. } => Some(mask),
        _ => None,
    }
}

pub(super) fn generated_mask_mut(geometry: &mut MaskGeometry) -> Option<&mut Option<MaskImage>> {
    match geometry {
        MaskGeometry::Ai { mask, .. }
        | MaskGeometry::Object { mask, .. }
        | MaskGeometry::Person { mask, .. } => Some(mask),
        _ => None,
    }
}

pub(super) fn mask_image_fingerprint(image: &MaskImage) -> u64 {
    let mut hasher = DefaultHasher::new();
    image.width.hash(&mut hasher);
    image.height.hash(&mut hasher);
    image.pixels.hash(&mut hasher);
    hasher.finish()
}

/// The 8-bit images of `depth` stored as mask assets: the sky, then the
/// normals' x and y.
pub(super) fn scene_depth_images(depth: &SceneDepth) -> [&MaskImage; 3] {
    [depth.sky(), depth.normals().x(), depth.normals().y()]
}

/// `disparity` as a 16-bit grayscale PNG.
pub(super) fn encode_disparity_png(disparity: &DisparityMap) -> Result<Arc<[u8]>, SidecarError> {
    let mut encoded = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut encoded, disparity.width(), disparity.height());
        encoder.set_color(png::ColorType::Grayscale);
        encoder.set_depth(png::BitDepth::Sixteen);
        encoder.set_compression(png::Compression::Balanced);
        let mut writer = encoder.write_header().map_err(|error| {
            SidecarError::Invalid(format!(
                "could not start disparity PNG compression: {error}"
            ))
        })?;
        // PNG stores 16-bit samples big-endian.
        let bytes: Vec<u8> = disparity
            .values()
            .iter()
            .flat_map(|value| value.to_be_bytes())
            .collect();
        writer.write_image_data(&bytes).map_err(|error| {
            SidecarError::Invalid(format!("could not compress scene disparity: {error}"))
        })?;
    }
    Ok(encoded.into())
}

pub(super) fn encode_mask_png(image: &MaskImage) -> Result<Arc<[u8]>, SidecarError> {
    let mut encoded = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut encoded, image.width, image.height);
        encoder.set_color(png::ColorType::Grayscale);
        encoder.set_depth(png::BitDepth::Eight);
        encoder.set_compression(png::Compression::Balanced);
        let mut writer = encoder.write_header().map_err(|error| {
            SidecarError::Invalid(format!("could not start mask PNG compression: {error}"))
        })?;
        writer.write_image_data(&image.pixels).map_err(|error| {
            SidecarError::Invalid(format!("could not compress generated mask: {error}"))
        })?;
    }
    Ok(encoded.into())
}

pub(super) fn extract_mask_assets(edits: &mut EditState) -> Result<MaskAssets, SidecarError> {
    let mut assets = Vec::<SidecarMaskAsset>::new();
    // Each distinct image with its asset index. The disparity asset precedes
    // the images, so asset indices are offset from positions in this list.
    let mut unique_images = Vec::<(MaskImage, usize)>::new();
    // Fingerprint to positions in `unique_images`.
    let mut buckets = HashMap::<u64, Vec<usize>>::new();
    let mut references = Vec::new();
    let mut decoded_asset_bytes = 0u64;
    let mut encoded_asset_bytes = 0u64;

    let masks = Arc::make_mut(&mut edits.masks);
    let scene_depth = masks.scene_depth.take();
    // The disparity comes first and is not deduplicated: no other asset is
    // 16-bit.
    let disparity_asset = match &scene_depth {
        Some(depth) => {
            let disparity = depth.disparity();
            let values = u64::try_from(disparity.values().len())
                .map_err(|_| SidecarError::TooLarge(u64::MAX))?;
            checked_add(&mut decoded_asset_bytes, 2 * values)?;
            if decoded_asset_bytes > MAX_DECODED_ASSET_BYTES {
                return invalid("scene depth exceeds the decoded asset memory safety limit");
            }
            let png = encode_disparity_png(disparity)?;
            checked_add(
                &mut encoded_asset_bytes,
                base64_json_string_bytes(png.len())?,
            )?;
            enforce_size_limit(encoded_asset_bytes, MAX_SIDECAR_BYTES)?;
            assets.push(SidecarMaskAsset {
                width: disparity.width(),
                height: disparity.height(),
                png,
            });
            Some(assets.len() - 1)
        }
        None => None,
    };

    let mut add_image = |image: MaskImage| -> Result<usize, SidecarError> {
        let fingerprint = mask_image_fingerprint(&image);
        if let Some((_, asset_index)) = buckets.get(&fingerprint).and_then(|candidates| {
            candidates
                .iter()
                .map(|position| &unique_images[*position])
                .find(|(unique, _)| *unique == image)
        }) {
            return Ok(*asset_index);
        }
        let pixels =
            u64::try_from(image.pixels.len()).map_err(|_| SidecarError::TooLarge(u64::MAX))?;
        checked_add(&mut decoded_asset_bytes, pixels)?;
        if decoded_asset_bytes > MAX_DECODED_ASSET_BYTES {
            return invalid("generated masks exceed the decoded asset memory safety limit");
        }
        let png = encode_mask_png(&image)?;
        checked_add(
            &mut encoded_asset_bytes,
            base64_json_string_bytes(png.len())?,
        )?;
        enforce_size_limit(encoded_asset_bytes, MAX_SIDECAR_BYTES)?;
        let asset_index = assets.len();
        assets.push(SidecarMaskAsset {
            width: image.width,
            height: image.height,
            png,
        });
        buckets
            .entry(fingerprint)
            .or_default()
            .push(unique_images.len());
        unique_images.push((image, asset_index));
        Ok(asset_index)
    };

    let scene_depth = match (scene_depth, disparity_asset) {
        (Some(depth), Some(disparity_asset)) => {
            let [sky, normal_x, normal_y] = scene_depth_images(&depth).map(MaskImage::clone);
            Some(SidecarSceneDepth {
                disparity_asset,
                sky_asset: add_image(sky)?,
                normal_assets: [add_image(normal_x)?, add_image(normal_y)?],
                camera: *depth.camera(),
            })
        }
        _ => None,
    };
    for (mask_index, mask) in masks.masks.iter_mut().enumerate() {
        for (component_index, component) in mask.components.iter_mut().enumerate() {
            let Some(image) = generated_mask_mut(&mut component.geometry).and_then(Option::take)
            else {
                continue;
            };
            let asset_index = add_image(image)?;
            references.push(SidecarMaskAssetRef {
                mask_index,
                component_index,
                asset_index,
            });
            if references.len() > MAX_MASK_ASSET_REFS {
                return invalid("edit contains too many generated mask references");
            }
        }
    }

    Ok(MaskAssets {
        assets,
        references,
        scene_depth,
    })
}

/// The samples of a grayscale PNG `asset` of `bit_depth` (8 or 16), as the
/// PNG stores them: one byte per pixel, or two big-endian bytes.
fn decode_gray_png(
    asset: &SidecarMaskAsset,
    bit_depth: png::BitDepth,
) -> Result<Vec<u8>, SidecarError> {
    let decoder = png::Decoder::new(Cursor::new(asset.png.as_ref()));
    let mut reader = decoder.read_info().map_err(|error| {
        SidecarError::Invalid(format!("could not read compressed mask PNG: {error}"))
    })?;
    let info = reader.info();
    if info.width != asset.width
        || info.height != asset.height
        || info.color_type != png::ColorType::Grayscale
        || info.bit_depth != bit_depth
        || info.animation_control.is_some()
    {
        return invalid("compressed mask PNG metadata does not match its asset");
    }

    let bytes_per_pixel = if bit_depth == png::BitDepth::Sixteen {
        2
    } else {
        1
    };
    let expected = usize::try_from(asset.width)
        .ok()
        .and_then(|width| {
            usize::try_from(asset.height)
                .ok()
                .and_then(|height| width.checked_mul(height))
        })
        .and_then(|pixels| pixels.checked_mul(bytes_per_pixel))
        .ok_or(SidecarError::TooLarge(u64::MAX))?;
    let output_size = reader
        .output_buffer_size()
        .ok_or(SidecarError::TooLarge(u64::MAX))?;
    if output_size != expected {
        return invalid("compressed mask PNG does not contain one grayscale sample per pixel");
    }
    let mut samples = Vec::new();
    samples
        .try_reserve_exact(expected)
        .map_err(|_| SidecarError::TooLarge(expected as u64))?;
    samples.resize(expected, 0);
    let output = reader.next_frame(&mut samples).map_err(|error| {
        SidecarError::Invalid(format!("could not decompress generated mask: {error}"))
    })?;
    if output.width != asset.width
        || output.height != asset.height
        || output.color_type != png::ColorType::Grayscale
        || output.bit_depth != bit_depth
        || output.buffer_size() != expected
    {
        return invalid("decompressed mask PNG dimensions or format are invalid");
    }
    Ok(samples)
}

pub(crate) fn decode_mask_png(asset: &SidecarMaskAsset) -> Result<MaskImage, SidecarError> {
    let pixels = decode_gray_png(asset, png::BitDepth::Eight)?;
    MaskImage::new(asset.width, asset.height, pixels)
        .ok_or_else(|| SidecarError::Invalid("decompressed mask pixels are invalid".to_owned()))
}

fn decode_disparity_png(asset: &SidecarMaskAsset) -> Result<DisparityMap, SidecarError> {
    let bytes = decode_gray_png(asset, png::BitDepth::Sixteen)?;
    let values = bytes
        .chunks_exact(2)
        .map(|pair| u16::from_be_bytes([pair[0], pair[1]]))
        .collect();
    DisparityMap::new(asset.width, asset.height, values)
        .ok_or_else(|| SidecarError::Invalid("decompressed disparity is invalid".to_owned()))
}

/// Restores the generated masks of a sidecar in the current layout from its
/// asset table, and returns its scene depth; every asset must be referenced.
pub(super) fn restore_mask_assets(
    edits: &mut EditState,
    assets: &[SidecarMaskAsset],
    references: &[SidecarMaskAssetRef],
    scene_depth: Option<&SidecarSceneDepth>,
) -> Result<Option<SceneDepth>, SidecarError> {
    if assets.len() > MAX_MASK_ASSETS || references.len() > MAX_MASK_ASSET_REFS {
        return invalid("sidecar contains too many generated mask assets");
    }
    if edits
        .masks
        .masks
        .iter()
        .flat_map(|mask| &mask.components)
        .any(|component| generated_mask(&component.geometry).is_some_and(Option::is_some))
    {
        return invalid("sidecar schema contains an inline generated mask");
    }

    let disparity_asset = scene_depth.map(|depth| depth.disparity_asset);
    let mut decoded_bytes = 0u64;
    for (index, asset) in assets.iter().enumerate() {
        let pixels = u64::from(asset.width)
            .checked_mul(u64::from(asset.height))
            .ok_or(SidecarError::TooLarge(u64::MAX))?;
        let pixels_usize = usize::try_from(pixels).map_err(|_| SidecarError::TooLarge(pixels))?;
        validate_image(asset.width, asset.height, pixels_usize, 1)?;
        // Disparity takes two bytes per pixel, and the scene depth derives a
        // one-byte selection map at its size.
        let bytes_per_pixel = if Some(index) == disparity_asset { 3 } else { 1 };
        checked_add(&mut decoded_bytes, pixels * bytes_per_pixel)?;
        if decoded_bytes > MAX_DECODED_ASSET_BYTES {
            return invalid("compressed mask assets exceed the decoded memory safety limit");
        }
    }

    let mut locations = HashSet::new();
    let mut referenced_assets = vec![false; assets.len()];
    if let Some(depth) = scene_depth {
        let images = [
            depth.sky_asset,
            depth.normal_assets[0],
            depth.normal_assets[1],
        ];
        if images.contains(&depth.disparity_asset) {
            return invalid("scene depth stores an image in its disparity asset");
        }
        for index in images.into_iter().chain([depth.disparity_asset]) {
            let Some(referenced) = referenced_assets.get_mut(index) else {
                return invalid("scene depth reference uses an invalid asset index");
            };
            *referenced = true;
        }
    }
    for reference in references {
        if Some(reference.asset_index) == disparity_asset {
            return invalid("generated mask reference targets the scene disparity");
        }
        let Some(asset_referenced) = referenced_assets.get_mut(reference.asset_index) else {
            return invalid("generated mask reference uses an invalid asset index");
        };
        let Some(component) = edits
            .masks
            .masks
            .get(reference.mask_index)
            .and_then(|mask| mask.components.get(reference.component_index))
        else {
            return invalid("generated mask reference uses an invalid component index");
        };
        if generated_mask(&component.geometry).is_none() {
            return invalid("generated mask reference targets an incompatible component");
        }
        if !locations.insert((reference.mask_index, reference.component_index)) {
            return invalid("sidecar contains duplicate references for a generated mask");
        }
        *asset_referenced = true;
    }
    if referenced_assets.iter().any(|referenced| !referenced) {
        return invalid("sidecar contains an unreferenced generated mask asset");
    }

    // The disparity is decoded on its own below.
    let decoded = assets
        .iter()
        .enumerate()
        .map(|(index, asset)| {
            (Some(index) != disparity_asset)
                .then(|| decode_mask_png(asset))
                .transpose()
        })
        .collect::<Result<Vec<_>, _>>()?;
    let image = |index: usize| {
        decoded[index]
            .clone()
            .ok_or_else(|| SidecarError::Invalid("mask asset is not an image".to_owned()))
    };
    let scene_depth = scene_depth
        .map(|depth| {
            if !depth.camera.is_valid() {
                return invalid("scene camera is invalid");
            }
            let normals = SceneNormals::new(
                image(depth.normal_assets[0])?,
                image(depth.normal_assets[1])?,
            )
            .ok_or_else(|| {
                SidecarError::Invalid("scene normal components differ in size".to_owned())
            })?;
            SceneDepth::new(
                decode_disparity_png(&assets[depth.disparity_asset])?,
                image(depth.sky_asset)?,
                normals,
                depth.camera,
            )
            .ok_or_else(|| SidecarError::Invalid("scene depth is invalid".to_owned()))
        })
        .transpose()?;
    let stack = Arc::make_mut(&mut edits.masks);
    for reference in references {
        let component =
            &mut stack.masks[reference.mask_index].components[reference.component_index];
        let slot = generated_mask_mut(&mut component.geometry)
            .ok_or_else(|| SidecarError::Invalid("generated mask target disappeared".to_owned()))?;
        *slot = Some(image(reference.asset_index)?);
    }
    Ok(scene_depth)
}
