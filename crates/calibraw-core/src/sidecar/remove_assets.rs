//! Remove/retouch patches stored as deduplicated, compressed PNG assets.

use super::*;

#[derive(Clone, Debug, PartialEq)]
struct RemoveAssetPayload {
    pub(super) width: u32,
    pub(super) height: u32,
    encoding: SidecarRemoveEncoding,
    pub(super) rgb: Arc<[u16]>,
    alpha: Arc<[u8]>,
}

fn remove_payload_fingerprint(payload: &RemoveAssetPayload) -> u64 {
    let mut hasher = DefaultHasher::new();
    payload.width.hash(&mut hasher);
    payload.height.hash(&mut hasher);
    payload.encoding.hash(&mut hasher);
    payload.rgb.hash(&mut hasher);
    payload.alpha.hash(&mut hasher);
    hasher.finish()
}

fn encode_remove_rgb_png(payload: &RemoveAssetPayload) -> Result<Arc<[u8]>, SidecarError> {
    let mut encoded = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut encoded, payload.width, payload.height);
        encoder.set_color(png::ColorType::Rgb);
        encoder.set_depth(png::BitDepth::Sixteen);
        encoder.set_compression(png::Compression::High);
        let mut writer = encoder.write_header().map_err(|error| {
            SidecarError::Invalid(format!("could not start retouch RGB compression: {error}"))
        })?;
        {
            let mut stream = writer.stream_writer_with_size(64 * 1024).map_err(|error| {
                SidecarError::Invalid(format!("could not stream retouch RGB compression: {error}"))
            })?;
            let values_per_row = payload.width as usize * 3;
            let mut row = Vec::new();
            row.try_reserve_exact(values_per_row.saturating_mul(2))
                .map_err(|_| SidecarError::TooLarge(values_per_row.saturating_mul(2) as u64))?;
            for values in payload.rgb.chunks_exact(values_per_row) {
                row.clear();
                for value in values {
                    row.extend_from_slice(&value.to_be_bytes());
                }
                stream.write_all(&row).map_err(|error| {
                    SidecarError::Invalid(format!("could not compress retouch RGB: {error}"))
                })?;
            }
            stream.finish().map_err(|error| {
                SidecarError::Invalid(format!("could not finish retouch RGB compression: {error}"))
            })?;
        }
        writer.finish().map_err(|error| {
            SidecarError::Invalid(format!("could not finalize retouch RGB PNG: {error}"))
        })?;
    }
    Ok(encoded.into())
}

fn encode_remove_alpha_png(payload: &RemoveAssetPayload) -> Result<Arc<[u8]>, SidecarError> {
    let mut encoded = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut encoded, payload.width, payload.height);
        encoder.set_color(png::ColorType::Grayscale);
        encoder.set_depth(png::BitDepth::Eight);
        encoder.set_compression(png::Compression::High);
        let mut writer = encoder.write_header().map_err(|error| {
            SidecarError::Invalid(format!(
                "could not start retouch alpha compression: {error}"
            ))
        })?;
        writer.write_image_data(&payload.alpha).map_err(|error| {
            SidecarError::Invalid(format!("could not compress retouch alpha: {error}"))
        })?;
    }
    Ok(encoded.into())
}

fn compressed_remove_payload(
    payload: &RemoveAssetPayload,
    fingerprint: u64,
    cache: &Arc<std::sync::OnceLock<RemovePatchSidecarCache>>,
) -> Result<RemovePatchSidecarCache, SidecarError> {
    if let Some(cached) = cache
        .get()
        .filter(|cached| cached.fingerprint == fingerprint)
    {
        return Ok(cached.clone());
    }
    let compressed = RemovePatchSidecarCache {
        fingerprint,
        rgb_png: encode_remove_rgb_png(payload)?,
        alpha_png: encode_remove_alpha_png(payload)?,
    };
    let _ = cache.set(compressed.clone());
    Ok(cache
        .get()
        .filter(|cached| cached.fingerprint == fingerprint)
        .cloned()
        .unwrap_or(compressed))
}

pub(super) fn extract_remove_assets(
    edits: &mut EditState,
) -> Result<(Vec<SidecarRemoveAsset>, Vec<SidecarRemoveAssetRef>), SidecarError> {
    let mut assets = Vec::<SidecarRemoveAsset>::new();
    let mut unique_payloads = Vec::<RemoveAssetPayload>::new();
    let mut buckets = HashMap::<u64, Vec<usize>>::new();
    let mut references = Vec::new();
    let mut decoded_asset_bytes = 0u64;
    let mut encoded_asset_bytes = 0u64;

    for (stroke_index, stroke) in Arc::make_mut(&mut edits.remove)
        .strokes
        .iter_mut()
        .enumerate()
    {
        for (patch_index, patch) in stroke.patches.iter_mut().enumerate() {
            let payload = RemoveAssetPayload {
                width: patch.bounds.width,
                height: patch.bounds.height,
                encoding: SidecarRemoveEncoding::Scene16f,
                rgb: Arc::clone(&patch.rgb_scene16f),
                alpha: Arc::clone(&patch.alpha),
            };
            let fingerprint = remove_payload_fingerprint(&payload);
            let existing = buckets.get(&fingerprint).and_then(|candidates| {
                candidates
                    .iter()
                    .copied()
                    .find(|index| unique_payloads[*index] == payload)
            });
            let asset_index = if let Some(index) = existing {
                let compressed = RemovePatchSidecarCache {
                    fingerprint,
                    rgb_png: Arc::clone(&assets[index].rgb_png),
                    alpha_png: Arc::clone(&assets[index].alpha_png),
                };
                let _ = patch.sidecar_cache.set(compressed);
                index
            } else {
                let pixels = u64::from(payload.width)
                    .checked_mul(u64::from(payload.height))
                    .ok_or(SidecarError::TooLarge(u64::MAX))?;
                let decoded_bytes = pixels
                    .checked_mul(7)
                    .ok_or(SidecarError::TooLarge(u64::MAX))?;
                decoded_asset_bytes = decoded_asset_bytes
                    .checked_add(decoded_bytes)
                    .ok_or(SidecarError::TooLarge(u64::MAX))?;
                if decoded_asset_bytes > MAX_DECODED_REMOVE_ASSET_BYTES {
                    return invalid("retouch patches exceed the decoded asset memory safety limit");
                }
                let compressed =
                    compressed_remove_payload(&payload, fingerprint, &patch.sidecar_cache)?;
                let rgb_bytes = base64_json_string_bytes(compressed.rgb_png.len())?;
                let alpha_bytes = base64_json_string_bytes(compressed.alpha_png.len())?;
                encoded_asset_bytes = encoded_asset_bytes
                    .checked_add(rgb_bytes)
                    .and_then(|bytes| bytes.checked_add(alpha_bytes))
                    .ok_or(SidecarError::TooLarge(u64::MAX))?;
                if encoded_asset_bytes > MAX_SIDECAR_BYTES {
                    return Err(SidecarError::TooLarge(encoded_asset_bytes));
                }
                let index = assets.len();
                assets.push(SidecarRemoveAsset {
                    width: payload.width,
                    height: payload.height,
                    encoding: payload.encoding,
                    rgb_png: compressed.rgb_png,
                    alpha_png: compressed.alpha_png,
                });
                unique_payloads.push(payload);
                buckets.entry(fingerprint).or_default().push(index);
                index
            };
            references.push(SidecarRemoveAssetRef {
                stroke_index,
                patch_index,
                asset_index,
            });
            if references.len() > MAX_REMOVE_ASSET_REFS {
                return invalid("edit contains too many retouch patch references");
            }
            patch.rgb_scene16f = Arc::from([]);
            patch.alpha = Arc::from([]);
        }
    }
    Ok((assets, references))
}

fn remove_png_decoder(
    bytes: &Arc<[u8]>,
    expected_output: usize,
) -> Result<png::Reader<Cursor<&[u8]>>, SidecarError> {
    let mut decoder = png::Decoder::new(Cursor::new(bytes.as_ref()));
    let decoder_limit = expected_output
        .checked_add(16 * 1024 * 1024)
        .ok_or(SidecarError::TooLarge(u64::MAX))?;
    decoder.set_limits(png::Limits {
        bytes: decoder_limit,
    });
    decoder.read_info().map_err(|error| {
        SidecarError::Invalid(format!("could not read compressed retouch PNG: {error}"))
    })
}

fn decode_remove_rgb_png(asset: &SidecarRemoveAsset) -> Result<Arc<[u16]>, SidecarError> {
    let pixels = asset.width as usize * asset.height as usize;
    let expected = pixels
        .checked_mul(6)
        .ok_or(SidecarError::TooLarge(u64::MAX))?;
    let mut reader = remove_png_decoder(&asset.rgb_png, expected)?;
    let info = reader.info();
    if info.width != asset.width
        || info.height != asset.height
        || info.color_type != png::ColorType::Rgb
        || info.bit_depth != png::BitDepth::Sixteen
        || info.animation_control.is_some()
        || reader.output_buffer_size() != Some(expected)
    {
        return invalid("compressed retouch RGB PNG metadata is invalid");
    }
    let mut bytes = vec![0u8; expected];
    let output = reader.next_frame(&mut bytes).map_err(|error| {
        SidecarError::Invalid(format!("could not decompress retouch RGB: {error}"))
    })?;
    if output.buffer_size() != expected {
        return invalid("decompressed retouch RGB byte count is invalid");
    }
    Ok(bytes
        .chunks_exact(2)
        .map(|bytes| u16::from_be_bytes([bytes[0], bytes[1]]))
        .collect::<Vec<_>>()
        .into())
}

fn decode_remove_alpha_png(asset: &SidecarRemoveAsset) -> Result<Arc<[u8]>, SidecarError> {
    let expected = asset.width as usize * asset.height as usize;
    let mut reader = remove_png_decoder(&asset.alpha_png, expected)?;
    let info = reader.info();
    if info.width != asset.width
        || info.height != asset.height
        || info.color_type != png::ColorType::Grayscale
        || info.bit_depth != png::BitDepth::Eight
        || info.animation_control.is_some()
        || reader.output_buffer_size() != Some(expected)
    {
        return invalid("compressed retouch alpha PNG metadata is invalid");
    }
    let mut alpha = vec![0u8; expected];
    let output = reader.next_frame(&mut alpha).map_err(|error| {
        SidecarError::Invalid(format!("could not decompress retouch alpha: {error}"))
    })?;
    if output.buffer_size() != expected {
        return invalid("decompressed retouch alpha byte count is invalid");
    }
    Ok(alpha.into())
}

#[derive(Clone)]
struct DecodedRemoveAsset {
    encoding: SidecarRemoveEncoding,
    pub(super) rgb: Arc<[u16]>,
    alpha: Arc<[u8]>,
    fingerprint: u64,
}

pub(super) fn restore_remove_assets(
    edits: &mut EditState,
    assets: &[SidecarRemoveAsset],
    references: &[SidecarRemoveAssetRef],
) -> Result<(), SidecarError> {
    if assets.len() > MAX_REMOVE_ASSET_REFS || references.len() > MAX_REMOVE_ASSET_REFS {
        return invalid("sidecar contains too many retouch patch assets");
    }
    let mut patch_count = 0usize;
    for stroke in &edits.remove.strokes {
        patch_count = patch_count
            .checked_add(stroke.patches.len())
            .ok_or(SidecarError::TooLarge(u64::MAX))?;
        for patch in &stroke.patches {
            if !patch.rgb_scene16f.is_empty() || !patch.alpha.is_empty() {
                return invalid("sidecar schema contains an inline retouch patch");
            }
        }
    }
    if references.len() != patch_count {
        return invalid("sidecar does not reference every retouch patch payload");
    }

    let mut decoded_bytes = 0u64;
    for asset in assets {
        if asset.width == 0 || asset.height == 0 || asset.width > 32_768 || asset.height > 32_768 {
            return invalid("compressed retouch patch has invalid dimensions");
        }
        let pixels = u64::from(asset.width)
            .checked_mul(u64::from(asset.height))
            .ok_or(SidecarError::TooLarge(u64::MAX))?;
        let bytes = pixels
            .checked_mul(7)
            .ok_or(SidecarError::TooLarge(u64::MAX))?;
        decoded_bytes = decoded_bytes
            .checked_add(bytes)
            .ok_or(SidecarError::TooLarge(u64::MAX))?;
        if decoded_bytes > MAX_DECODED_REMOVE_ASSET_BYTES {
            return invalid("compressed retouch patches exceed the decoded memory safety limit");
        }
    }

    let mut locations = HashSet::new();
    let mut referenced_assets = vec![false; assets.len()];
    for reference in references {
        let Some(asset_referenced) = referenced_assets.get_mut(reference.asset_index) else {
            return invalid("retouch patch reference uses an invalid asset index");
        };
        let Some(patch) = edits
            .remove
            .strokes
            .get(reference.stroke_index)
            .and_then(|stroke| stroke.patches.get(reference.patch_index))
        else {
            return invalid("retouch patch reference uses an invalid patch index");
        };
        let asset = &assets[reference.asset_index];
        if patch.bounds.width != asset.width || patch.bounds.height != asset.height {
            return invalid("retouch patch reference dimensions do not match its asset");
        }
        if !locations.insert((reference.stroke_index, reference.patch_index)) {
            return invalid("sidecar contains duplicate references for a retouch patch");
        }
        *asset_referenced = true;
    }
    if referenced_assets.iter().any(|referenced| !referenced) {
        return invalid("sidecar contains an unreferenced retouch patch asset");
    }

    let decoded = assets
        .iter()
        .map(|asset| {
            let rgb = decode_remove_rgb_png(asset)?;
            let alpha = decode_remove_alpha_png(asset)?;
            let fingerprint = remove_payload_fingerprint(&RemoveAssetPayload {
                width: asset.width,
                height: asset.height,
                encoding: asset.encoding,
                rgb: Arc::clone(&rgb),
                alpha: Arc::clone(&alpha),
            });
            Ok(DecodedRemoveAsset {
                encoding: asset.encoding,
                rgb,
                alpha,
                fingerprint,
            })
        })
        .collect::<Result<Vec<_>, SidecarError>>()?;
    let remove = Arc::make_mut(&mut edits.remove);
    for reference in references {
        let asset = &assets[reference.asset_index];
        let decoded = &decoded[reference.asset_index];
        let patch = &mut remove.strokes[reference.stroke_index].patches[reference.patch_index];
        match decoded.encoding {
            SidecarRemoveEncoding::Scene16f => {
                patch.rgb_scene16f = Arc::clone(&decoded.rgb);
            }
        }
        patch.alpha = Arc::clone(&decoded.alpha);
        let _ = patch.sidecar_cache.set(RemovePatchSidecarCache {
            fingerprint: decoded.fingerprint,
            rgb_png: Arc::clone(&asset.rgb_png),
            alpha_png: Arc::clone(&asset.alpha_png),
        });
    }
    Ok(())
}
