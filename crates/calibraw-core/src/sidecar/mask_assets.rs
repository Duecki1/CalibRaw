//! Generated mask images stored as deduplicated PNG assets.

use super::*;

pub(super) fn generated_mask(geometry: &MaskGeometry) -> Option<&Option<MaskImage>> {
    match geometry {
        MaskGeometry::Ai { mask, .. } | MaskGeometry::Object { mask, .. } => Some(mask),
        MaskGeometry::DepthRange { depth, .. } => Some(depth),
        _ => None,
    }
}

pub(super) fn generated_mask_mut(geometry: &mut MaskGeometry) -> Option<&mut Option<MaskImage>> {
    match geometry {
        MaskGeometry::Ai { mask, .. } | MaskGeometry::Object { mask, .. } => Some(mask),
        MaskGeometry::DepthRange { depth, .. } => Some(depth),
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
    let mut unique_images = Vec::<MaskImage>::new();
    let mut buckets = HashMap::<u64, Vec<usize>>::new();
    let mut references = Vec::new();
    let mut decoded_asset_bytes = 0u64;
    let mut encoded_asset_bytes = 0u64;

    let mut add_image = |image: MaskImage| -> Result<usize, SidecarError> {
        let fingerprint = mask_image_fingerprint(&image);
        if let Some(index) = buckets.get(&fingerprint).and_then(|candidates| {
            candidates
                .iter()
                .copied()
                .find(|index| unique_images[*index] == image)
        }) {
            return Ok(index);
        }
        let pixels =
            u64::try_from(image.pixels.len()).map_err(|_| SidecarError::TooLarge(u64::MAX))?;
        checked_add(&mut decoded_asset_bytes, pixels)?;
        if decoded_asset_bytes > MAX_DECODED_MASK_ASSET_BYTES {
            return invalid("generated masks exceed the decoded asset memory safety limit");
        }
        let png = encode_mask_png(&image)?;
        checked_add(
            &mut encoded_asset_bytes,
            base64_json_string_bytes(png.len())?,
        )?;
        enforce_size_limit(encoded_asset_bytes, MAX_SIDECAR_BYTES)?;
        let index = assets.len();
        assets.push(SidecarMaskAsset {
            width: image.width,
            height: image.height,
            png,
        });
        unique_images.push(image);
        buckets.entry(fingerprint).or_default().push(index);
        Ok(index)
    };

    let masks = Arc::make_mut(&mut edits.masks);
    let scene_depth_asset = masks.scene_depth.take().map(&mut add_image).transpose()?;
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
        scene_depth_asset,
    })
}

fn decode_mask_png(asset: &SidecarMaskAsset) -> Result<MaskImage, SidecarError> {
    let decoder = png::Decoder::new(Cursor::new(asset.png.as_ref()));
    let mut reader = decoder.read_info().map_err(|error| {
        SidecarError::Invalid(format!("could not read compressed mask PNG: {error}"))
    })?;
    let info = reader.info();
    if info.width != asset.width
        || info.height != asset.height
        || info.color_type != png::ColorType::Grayscale
        || info.bit_depth != png::BitDepth::Eight
        || info.animation_control.is_some()
    {
        return invalid("compressed mask PNG metadata does not match its asset");
    }

    let expected = usize::try_from(asset.width)
        .ok()
        .and_then(|width| {
            usize::try_from(asset.height)
                .ok()
                .and_then(|height| width.checked_mul(height))
        })
        .ok_or(SidecarError::TooLarge(u64::MAX))?;
    let output_size = reader
        .output_buffer_size()
        .ok_or(SidecarError::TooLarge(u64::MAX))?;
    if output_size != expected {
        return invalid("compressed mask PNG does not contain one grayscale byte per pixel");
    }
    let mut pixels = Vec::new();
    pixels
        .try_reserve_exact(expected)
        .map_err(|_| SidecarError::TooLarge(expected as u64))?;
    pixels.resize(expected, 0);
    let output = reader.next_frame(&mut pixels).map_err(|error| {
        SidecarError::Invalid(format!("could not decompress generated mask: {error}"))
    })?;
    if output.width != asset.width
        || output.height != asset.height
        || output.color_type != png::ColorType::Grayscale
        || output.bit_depth != png::BitDepth::Eight
        || output.buffer_size() != expected
    {
        return invalid("decompressed mask PNG dimensions or format are invalid");
    }
    MaskImage::new(asset.width, asset.height, pixels)
        .ok_or_else(|| SidecarError::Invalid("decompressed mask pixels are invalid".to_owned()))
}

pub(super) fn restore_mask_assets(
    edits: &mut EditState,
    assets: &[SidecarMaskAsset],
    references: &[SidecarMaskAssetRef],
    scene_depth_asset: Option<usize>,
) -> Result<(), SidecarError> {
    if assets.len() > MAX_MASK_ASSETS || references.len() > MAX_MASK_ASSET_REFS {
        return invalid("sidecar contains too many generated mask assets");
    }

    let mut decoded_bytes = 0u64;
    if let Some(image) = &edits.masks.scene_depth {
        validate_image(image.width, image.height, image.pixels.len(), 1)?;
        checked_add(&mut decoded_bytes, image.pixels.len() as u64)?;
    }
    for mask in &edits.masks.masks {
        for component in &mask.components {
            if let MaskGeometry::DepthRange {
                depth: Some(image), ..
            } = &component.geometry
            {
                // migration: remove in v2.0.0. Depth sidecars up to v1.1 stored
                // inline pixels. Read them losslessly; the next save moves them
                // into the shared PNG asset table. Listed in `crate::migrations`.
                validate_image(image.width, image.height, image.pixels.len(), 1)?;
                decoded_bytes = decoded_bytes
                    .checked_add(image.pixels.len() as u64)
                    .ok_or(SidecarError::TooLarge(u64::MAX))?;
                if decoded_bytes > MAX_DECODED_MASK_ASSET_BYTES {
                    return invalid("generated masks exceed the decoded memory safety limit");
                }
            } else if generated_mask(&component.geometry).is_some_and(Option::is_some) {
                return invalid("sidecar schema contains an inline generated mask");
            }
        }
    }

    for asset in assets {
        let pixels = u64::from(asset.width)
            .checked_mul(u64::from(asset.height))
            .ok_or(SidecarError::TooLarge(u64::MAX))?;
        let pixels_usize = usize::try_from(pixels).map_err(|_| SidecarError::TooLarge(pixels))?;
        validate_image(asset.width, asset.height, pixels_usize, 1)?;
        decoded_bytes = decoded_bytes
            .checked_add(pixels)
            .ok_or(SidecarError::TooLarge(u64::MAX))?;
        if decoded_bytes > MAX_DECODED_MASK_ASSET_BYTES {
            return invalid("compressed mask assets exceed the decoded memory safety limit");
        }
    }

    let mut locations = HashSet::new();
    let mut referenced_assets = vec![false; assets.len()];
    if let Some(index) = scene_depth_asset {
        if edits.masks.scene_depth.is_some() {
            return invalid("scene depth has both inline pixels and an asset reference");
        }
        let Some(referenced) = referenced_assets.get_mut(index) else {
            return invalid("scene depth reference uses an invalid asset index");
        };
        *referenced = true;
    }
    for reference in references {
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
        if generated_mask(&component.geometry).is_some_and(Option::is_some) {
            return invalid("generated mask has both inline pixels and an asset reference");
        }
        if !locations.insert((reference.mask_index, reference.component_index)) {
            return invalid("sidecar contains duplicate references for a generated mask");
        }
        *asset_referenced = true;
    }
    if referenced_assets.iter().any(|referenced| !referenced) {
        return invalid("sidecar contains an unreferenced generated mask asset");
    }

    let decoded = assets
        .iter()
        .map(decode_mask_png)
        .collect::<Result<Vec<_>, _>>()?;
    let stack = Arc::make_mut(&mut edits.masks);
    if let Some(index) = scene_depth_asset {
        stack.scene_depth = Some(decoded[index].clone());
    }
    let masks = &mut stack.masks;
    for reference in references {
        let component = &mut masks[reference.mask_index].components[reference.component_index];
        let slot = generated_mask_mut(&mut component.geometry)
            .ok_or_else(|| SidecarError::Invalid("generated mask target disappeared".to_owned()))?;
        *slot = Some(decoded[reference.asset_index].clone());
    }
    Ok(())
}
