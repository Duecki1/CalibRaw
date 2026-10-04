//! Saved denoise results: the cache file, its manifest and source fingerprints.

use super::*;

pub fn load_saved_result(path: &Path, raw: &LoadedRaw) -> Result<Option<AiDenoisedImage>> {
    let file = match File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error).with_context(|| format!("open {}", path.display())),
    };
    let mut archive = ZipArchive::new(file)
        .with_context(|| format!("read AI-denoise result archive {}", path.display()))?;
    let header = {
        let mut entry = archive
            .by_name(RESULT_FILE_MANIFEST)
            .context("AI-denoise result has no manifest")?;
        anyhow::ensure!(
            entry.size() == RESULT_FILE_HEADER_BYTES as u64,
            "AI-denoise result manifest has an unexpected size"
        );
        let mut header = [0u8; RESULT_FILE_HEADER_BYTES];
        entry
            .read_exact(&mut header)
            .context("read AI-denoise result manifest")?;
        header
    };
    let manifest = ResultCacheManifest::decode(&header)?;
    anyhow::ensure!(
        manifest.width == raw.width && manifest.height == raw.height,
        "AI-denoise result dimensions do not match the RAW"
    );
    anyhow::ensure!(
        manifest.cfa_kind == cfa_cache_code(raw.cfa_kind),
        "AI-denoise result CFA type does not match the RAW"
    );
    let channels = match raw.cfa_kind {
        CfaKind::Bayer => 1,
        CfaKind::XTrans => 3,
    };
    let expected_elements = u64::from(raw.width)
        .checked_mul(u64::from(raw.height))
        .and_then(|pixels| pixels.checked_mul(channels))
        .and_then(|elements| usize::try_from(elements).ok())
        .context("AI-denoise result dimensions overflow")?;
    let expected_bytes = expected_elements
        .checked_mul(std::mem::size_of::<u16>())
        .context("AI-denoise result byte count overflow")?;
    anyhow::ensure!(
        manifest.payload_bytes == expected_bytes as u64,
        "AI-denoise result payload size does not match the RAW"
    );
    anyhow::ensure!(
        manifest.source_sha256 == source_fingerprint(raw, None)?,
        "AI-denoise result belongs to a different RAW reconstruction"
    );

    let mut payload = vec![0u16; expected_elements];
    {
        let mut entry = archive
            .by_name(RESULT_FILE_PAYLOAD)
            .context("AI-denoise result has no scene payload")?;
        anyhow::ensure!(
            entry.size() == expected_bytes as u64,
            "AI-denoise result scene payload has an unexpected size"
        );
        entry
            .read_exact(bytemuck::cast_slice_mut(&mut payload))
            .context("read AI-denoise result scene payload")?;
        let mut trailing = [0u8; 1];
        anyhow::ensure!(
            entry.read(&mut trailing)? == 0,
            "AI-denoise result scene payload contains trailing data"
        );
    }
    let actual_payload = ring::digest::digest(&SHA256, bytemuck::cast_slice(&payload));
    anyhow::ensure!(
        actual_payload.as_ref() == manifest.payload_sha256,
        "AI-denoise result scene checksum does not match"
    );
    match raw.cfa_kind {
        CfaKind::Bayer => AiDenoisedImage::new_bayer_cfa(raw.width, raw.height, payload),
        CfaKind::XTrans => AiDenoisedImage::new(raw.width, raw.height, payload),
    }
    .map(Some)
}

pub fn save_result(
    path: &Path,
    raw: &LoadedRaw,
    image: &AiDenoisedImage,
    cancellation: &AtomicBool,
) -> Result<()> {
    anyhow::ensure!(
        image.is_valid_for(raw.width, raw.height),
        "cannot save an AI-denoise result with mismatched dimensions"
    );
    ensure_not_cancelled(cancellation)?;
    let source_sha256 = source_fingerprint(raw, Some(cancellation))?;
    anyhow::ensure!(
        matches!(raw.cfa_kind, CfaKind::Bayer) == image.bayer_cfa().is_some(),
        "cannot save an AI-denoise payload for a different CFA type"
    );
    let payload = bytemuck::cast_slice(image.payload());
    let payload_sha256 = digest_cancelable(payload, Some(cancellation))?;
    let manifest = ResultCacheManifest {
        width: raw.width,
        height: raw.height,
        cfa_kind: cfa_cache_code(raw.cfa_kind),
        payload_bytes: payload.len() as u64,
        source_sha256,
        payload_sha256,
    }
    .encode();

    let parent = path
        .parent()
        .context("AI-denoise result path has no parent directory")?;
    fs::create_dir_all(parent)
        .with_context(|| format!("create AI-denoise result directory {}", parent.display()))?;
    write_atomically(path, |file| -> Result<()> {
        let mut archive = ZipWriter::new(file);
        let stored = FileOptions::default().compression_method(CompressionMethod::Stored);
        archive
            .start_file(RESULT_FILE_MANIFEST, stored)
            .context("start AI-denoise result manifest")?;
        archive
            .write_all(&manifest)
            .context("write AI-denoise result manifest")?;
        let compressed = FileOptions::default()
            .compression_method(CompressionMethod::Deflated)
            .compression_level(Some(3));
        archive
            .start_file(RESULT_FILE_PAYLOAD, compressed)
            .context("start AI-denoise result scene payload")?;
        for chunk in payload.chunks(RESULT_FILE_IO_CHUNK) {
            ensure_not_cancelled(cancellation)?;
            archive
                .write_all(chunk)
                .context("write AI-denoise result scene payload")?;
        }
        archive.finish().context("finalize AI-denoise result")?;
        ensure_not_cancelled(cancellation)
    })
    .with_context(|| format!("write AI-denoise result {}", path.display()))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct ResultCacheManifest {
    pub(super) width: u32,
    pub(super) height: u32,
    pub(super) cfa_kind: u32,
    payload_bytes: u64,
    source_sha256: [u8; 32],
    payload_sha256: [u8; 32],
}

impl ResultCacheManifest {
    fn encode(self) -> [u8; RESULT_FILE_HEADER_BYTES] {
        let mut bytes = [0u8; RESULT_FILE_HEADER_BYTES];
        bytes[0..8].copy_from_slice(&RESULT_FILE_MAGIC);
        bytes[8..12].copy_from_slice(&RESULT_FILE_VERSION.to_le_bytes());
        bytes[12..16].copy_from_slice(&self.width.to_le_bytes());
        bytes[16..20].copy_from_slice(&self.height.to_le_bytes());
        bytes[20..24].copy_from_slice(&self.cfa_kind.to_le_bytes());
        bytes[24..32].copy_from_slice(&self.payload_bytes.to_le_bytes());
        bytes[32..64].copy_from_slice(&self.source_sha256);
        bytes[64..96].copy_from_slice(&self.payload_sha256);
        bytes
    }

    fn decode(bytes: &[u8; RESULT_FILE_HEADER_BYTES]) -> Result<Self> {
        anyhow::ensure!(
            bytes[0..8] == RESULT_FILE_MAGIC,
            "invalid AI-denoise result magic"
        );
        let read_u32 = |offset: usize| {
            u32::from_le_bytes(bytes[offset..offset + 4].try_into().expect("fixed header"))
        };
        let version = read_u32(8);
        anyhow::ensure!(
            version == RESULT_FILE_VERSION,
            "AI-denoise result format version {version} is stale"
        );
        Ok(Self {
            width: read_u32(12),
            height: read_u32(16),
            cfa_kind: read_u32(20),
            payload_bytes: u64::from_le_bytes(bytes[24..32].try_into().expect("fixed header")),
            source_sha256: bytes[32..64].try_into().expect("fixed header"),
            payload_sha256: bytes[64..96].try_into().expect("fixed header"),
        })
    }
}

fn cfa_cache_code(kind: CfaKind) -> u32 {
    match kind {
        CfaKind::Bayer => 1,
        CfaKind::XTrans => 2,
    }
}

fn source_fingerprint(raw: &LoadedRaw, cancellation: Option<&AtomicBool>) -> Result<[u8; 32]> {
    let mut digest = Sha256Context::new(&SHA256);
    digest.update(b"CalibRaw RawNIND source fingerprint v1\0");
    digest.update(&raw.width.to_le_bytes());
    digest.update(&raw.height.to_le_bytes());
    digest.update(&cfa_cache_code(raw.cfa_kind).to_le_bytes());
    update_digest_cancelable(
        &mut digest,
        bytemuck::cast_slice(raw.raw_pixels.as_slice()),
        cancellation,
    )?;
    let (color_width, color_height, colors) = raw.color_indices.storage_parts();
    digest.update(&color_width.to_le_bytes());
    digest.update(&color_height.to_le_bytes());
    update_digest_cancelable(&mut digest, colors, cancellation)?;
    let (black_width, black_height, blacks) = raw.black_levels_per_pixel.storage_parts();
    digest.update(&black_width.to_le_bytes());
    digest.update(&black_height.to_le_bytes());
    update_digest_cancelable(&mut digest, bytemuck::cast_slice(blacks), cancellation)?;
    digest.update(bytemuck::bytes_of(&raw.wb_coeffs));
    digest.update(bytemuck::bytes_of(&raw.cam_to_srgb));
    digest.update(bytemuck::bytes_of(&raw.black_levels));
    digest.update(bytemuck::bytes_of(&raw.white_levels));
    Ok(digest
        .finish()
        .as_ref()
        .try_into()
        .expect("SHA-256 is always 32 bytes"))
}

fn digest_cancelable(bytes: &[u8], cancellation: Option<&AtomicBool>) -> Result<[u8; 32]> {
    let mut digest = Sha256Context::new(&SHA256);
    update_digest_cancelable(&mut digest, bytes, cancellation)?;
    Ok(digest
        .finish()
        .as_ref()
        .try_into()
        .expect("SHA-256 is always 32 bytes"))
}

fn update_digest_cancelable(
    digest: &mut Sha256Context,
    bytes: &[u8],
    cancellation: Option<&AtomicBool>,
) -> Result<()> {
    for chunk in bytes.chunks(RESULT_FILE_IO_CHUNK) {
        if let Some(cancellation) = cancellation {
            ensure_not_cancelled(cancellation)?;
        }
        digest.update(chunk);
    }
    Ok(())
}

pub fn models_are_verified(model_dir: &Path) -> bool {
    verify_artifact(&model_dir.join("model_bayer.onnx"), BAYER_MODEL_ARTIFACT).is_ok()
        && verify_artifact(&model_dir.join("model_linear.onnx"), LINEAR_MODEL_ARTIFACT).is_ok()
}
