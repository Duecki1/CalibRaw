//! Bounding sidecar size before encoding: estimates and preflight checks.

use super::*;

pub(super) struct CappedVec {
    pub(super) bytes: Vec<u8>,
    limit: u64,
    pub(super) limit_reached: bool,
}

impl CappedVec {
    pub(super) fn new(limit: u64) -> Self {
        Self {
            bytes: Vec::new(),
            limit,
            limit_reached: false,
        }
    }
}

impl Write for CappedVec {
    fn write(&mut self, buffer: &[u8]) -> std::io::Result<usize> {
        let next = self.bytes.len() as u64 + buffer.len() as u64;
        if next > self.limit {
            self.limit_reached = true;
            return Err(std::io::Error::other("sidecar size limit reached"));
        }
        self.bytes.extend_from_slice(buffer);
        Ok(buffer.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

pub fn preflight_mask_change(masks: &MaskStack) -> Result<(), SidecarError> {
    preflight_sidecar_dynamic_data_with_limit(masks, MAX_SIDECAR_BYTES)
}

pub(super) fn preflight_sidecar_dynamic_data_with_limit(
    masks: &MaskStack,
    limit: u64,
) -> Result<(), SidecarError> {
    validate_scene_depth(masks)?;
    let conservative = estimate_sidecar_bytes(masks)?;
    if conservative <= limit {
        return Ok(());
    }
    let measured = measure_sidecar_dynamic_bytes(masks)?;
    enforce_size_limit(measured, limit)
}

pub(super) fn enforce_size_limit(estimated: u64, limit: u64) -> Result<(), SidecarError> {
    if estimated > limit {
        Err(SidecarError::TooLarge(estimated))
    } else {
        Ok(())
    }
}

pub(super) fn estimate_sidecar_bytes(masks: &MaskStack) -> Result<u64, SidecarError> {
    const DOCUMENT_HEADROOM: u64 = 1024 * 1024;
    const MASK_HEADROOM: u64 = 16 * 1024;
    const COMPONENT_HEADROOM: u64 = 2 * 1024;
    const BRUSH_DAB_HEADROOM: u64 = 256;
    const PATH_POINT_HEADROOM: u64 = 192;
    const OBJECT_STROKE_HEADROOM: u64 = 128;
    const OBJECT_POINT_HEADROOM: u64 = 96;
    const MASK_PNG_FIXED_HEADROOM: u64 = 64 * 1024;

    let mut estimated = DOCUMENT_HEADROOM;
    checked_add_scaled(
        &mut estimated,
        masks.subject_refinement.dabs.len(),
        BRUSH_DAB_HEADROOM,
    )?;
    let mut unique_images = Vec::<&MaskImage>::new();
    let mut image_buckets = HashMap::<u64, Vec<usize>>::new();
    if let Some(image) = &masks.scene_depth {
        add_unique_mask_asset_bound(
            &mut estimated,
            image,
            &mut unique_images,
            &mut image_buckets,
            MASK_PNG_FIXED_HEADROOM,
        )?;
    }
    for mask in &masks.masks {
        checked_add(&mut estimated, MASK_HEADROOM)?;
        checked_add(&mut estimated, escaped_json_string_bound(&mask.name)?)?;
        for component in &mask.components {
            checked_add(&mut estimated, COMPONENT_HEADROOM)?;
            checked_add(&mut estimated, escaped_json_string_bound(&component.name)?)?;
            match &component.geometry {
                MaskGeometry::Brush { dabs, .. } => {
                    checked_add_scaled(&mut estimated, dabs.len(), BRUSH_DAB_HEADROOM)?
                }
                MaskGeometry::Path { points, .. } => {
                    checked_add_scaled(&mut estimated, points.len(), PATH_POINT_HEADROOM)?
                }
                MaskGeometry::Ai {
                    mask: Some(image), ..
                }
                | MaskGeometry::DepthRange {
                    depth: Some(image), ..
                } => add_unique_mask_asset_bound(
                    &mut estimated,
                    image,
                    &mut unique_images,
                    &mut image_buckets,
                    MASK_PNG_FIXED_HEADROOM,
                )?,
                MaskGeometry::Object { mask, strokes, .. } => {
                    if let Some(image) = mask {
                        add_unique_mask_asset_bound(
                            &mut estimated,
                            image,
                            &mut unique_images,
                            &mut image_buckets,
                            MASK_PNG_FIXED_HEADROOM,
                        )?;
                    }
                    checked_add_scaled(&mut estimated, strokes.len(), OBJECT_STROKE_HEADROOM)?;
                    for stroke in strokes {
                        checked_add_scaled(
                            &mut estimated,
                            stroke.points.len(),
                            OBJECT_POINT_HEADROOM,
                        )?;
                    }
                }
                _ => {}
            }
        }
    }

    Ok(estimated)
}

pub(super) fn measure_sidecar_dynamic_bytes(masks: &MaskStack) -> Result<u64, SidecarError> {
    const DOCUMENT_HEADROOM: u64 = 1024 * 1024;
    const MASK_HEADROOM: u64 = 16 * 1024;
    const COMPONENT_HEADROOM: u64 = 2 * 1024;
    const OBJECT_STROKE_HEADROOM: u64 = 128;
    const OBJECT_POINT_HEADROOM: u64 = 96;
    const BRUSH_DAB_HEADROOM: u64 = 256;
    const PATH_POINT_HEADROOM: u64 = 192;

    let mut measured = DOCUMENT_HEADROOM;
    checked_add_scaled(
        &mut measured,
        masks.subject_refinement.dabs.len(),
        BRUSH_DAB_HEADROOM,
    )?;
    let mut unique_images = Vec::<&MaskImage>::new();
    let mut image_buckets = HashMap::<u64, Vec<usize>>::new();
    if let Some(image) = &masks.scene_depth {
        add_unique_mask_asset_measured(
            &mut measured,
            image,
            &mut unique_images,
            &mut image_buckets,
        )?;
    }
    for mask in &masks.masks {
        checked_add(&mut measured, MASK_HEADROOM)?;
        checked_add(&mut measured, escaped_json_string_bound(&mask.name)?)?;
        for component in &mask.components {
            checked_add(&mut measured, COMPONENT_HEADROOM)?;
            checked_add(&mut measured, escaped_json_string_bound(&component.name)?)?;
            match &component.geometry {
                MaskGeometry::Brush { dabs, .. } => {
                    checked_add_scaled(&mut measured, dabs.len(), BRUSH_DAB_HEADROOM)?
                }
                MaskGeometry::Path { points, .. } => {
                    checked_add_scaled(&mut measured, points.len(), PATH_POINT_HEADROOM)?
                }
                MaskGeometry::Ai {
                    mask: Some(image), ..
                }
                | MaskGeometry::DepthRange {
                    depth: Some(image), ..
                } => add_unique_mask_asset_measured(
                    &mut measured,
                    image,
                    &mut unique_images,
                    &mut image_buckets,
                )?,
                MaskGeometry::Object { mask, strokes, .. } => {
                    if let Some(image) = mask {
                        add_unique_mask_asset_measured(
                            &mut measured,
                            image,
                            &mut unique_images,
                            &mut image_buckets,
                        )?;
                    }
                    checked_add_scaled(&mut measured, strokes.len(), OBJECT_STROKE_HEADROOM)?;
                    for stroke in strokes {
                        checked_add_scaled(
                            &mut measured,
                            stroke.points.len(),
                            OBJECT_POINT_HEADROOM,
                        )?;
                    }
                }
                _ => {}
            }
        }
    }

    Ok(measured)
}

fn add_unique_mask_asset_measured<'a>(
    measured: &mut u64,
    image: &'a MaskImage,
    unique_images: &mut Vec<&'a MaskImage>,
    buckets: &mut HashMap<u64, Vec<usize>>,
) -> Result<(), SidecarError> {
    let fingerprint = mask_image_fingerprint(image);
    if buckets.get(&fingerprint).is_some_and(|candidates| {
        candidates
            .iter()
            .any(|index| *unique_images[*index] == *image)
    }) {
        return Ok(());
    }

    let index = unique_images.len();
    unique_images.push(image);
    buckets.entry(fingerprint).or_default().push(index);
    let png = encode_mask_png(image)?;
    checked_add(measured, base64_json_string_bytes(png.len())?)
}

fn add_unique_mask_asset_bound<'a>(
    estimated: &mut u64,
    image: &'a MaskImage,
    unique_images: &mut Vec<&'a MaskImage>,
    buckets: &mut HashMap<u64, Vec<usize>>,
    fixed_headroom: u64,
) -> Result<(), SidecarError> {
    let fingerprint = mask_image_fingerprint(image);
    if buckets.get(&fingerprint).is_some_and(|candidates| {
        candidates
            .iter()
            .any(|index| *unique_images[*index] == *image)
    }) {
        return Ok(());
    }

    let index = unique_images.len();
    unique_images.push(image);
    buckets.entry(fingerprint).or_default().push(index);
    let raw_bytes =
        u64::try_from(image.pixels.len()).map_err(|_| SidecarError::TooLarge(u64::MAX))?;
    let png_bound = raw_bytes
        .checked_add(raw_bytes.div_ceil(64))
        .and_then(|bytes| bytes.checked_add(fixed_headroom))
        .ok_or(SidecarError::TooLarge(u64::MAX))?;
    let png_bound = usize::try_from(png_bound).map_err(|_| SidecarError::TooLarge(png_bound))?;
    checked_add(estimated, base64_json_string_bytes(png_bound)?)
}

pub(super) fn checked_add(total: &mut u64, value: u64) -> Result<(), SidecarError> {
    *total = total
        .checked_add(value)
        .ok_or(SidecarError::TooLarge(u64::MAX))?;
    Ok(())
}

fn checked_add_scaled(
    total: &mut u64,
    count: usize,
    bytes_per_item: u64,
) -> Result<(), SidecarError> {
    let count = u64::try_from(count).map_err(|_| SidecarError::TooLarge(u64::MAX))?;
    let bytes = count
        .checked_mul(bytes_per_item)
        .ok_or(SidecarError::TooLarge(u64::MAX))?;
    checked_add(total, bytes)
}

fn escaped_json_string_bound(value: &str) -> Result<u64, SidecarError> {
    let bytes = u64::try_from(value.len()).map_err(|_| SidecarError::TooLarge(u64::MAX))?;
    bytes
        .checked_mul(6)
        .and_then(|bytes| bytes.checked_add(2))
        .ok_or(SidecarError::TooLarge(u64::MAX))
}

pub(super) fn base64_json_string_bytes(byte_count: usize) -> Result<u64, SidecarError> {
    let byte_count = u64::try_from(byte_count).map_err(|_| SidecarError::TooLarge(u64::MAX))?;
    byte_count
        .div_ceil(3)
        .checked_mul(4)
        .and_then(|bytes| bytes.checked_add(2))
        .ok_or(SidecarError::TooLarge(u64::MAX))
}
