//! Lens names that a camera's maker note records only as an ID, looked up in
//! Rawler's lens table. Pentax bodies write no EXIF lens name at all, and OM
//! Digital bodies name newer versions of lenses that Lensfun files under the
//! earlier version's name.

use super::LoadedRaw;
use rawler::lens::{get_lenses, LensDescription};

/// Rawler's mount names for the lenses these IDs identify.
const PENTAX_MOUNT: &str = "k-mount";
const MFT_MOUNT: &str = "MFT-mount";

/// Records `lens` as the maker note's lens, and as the lens itself when the
/// EXIF data names none, so the Info panel and exports show it too.
pub(super) fn record(raw: &mut LoadedRaw, lens: Option<&LensDescription>) {
    let Some(lens) = lens.filter(|lens| !lens.lens_name.trim().is_empty()) else {
        return;
    };
    if raw.lens_model.trim().is_empty() {
        raw.lens_make.clone_from(&lens.lens_make);
        raw.lens_model.clone_from(&lens.lens_model);
    }
    raw.makernote_lens_model.clone_from(&lens.lens_name);
}

/// The lens a LibRaw maker-note `LensID` names, read as Pentax or Olympus
/// encodes it by `camera_make`. Other makers' IDs are not looked up.
pub(super) fn from_libraw_id(
    camera_make: &str,
    lens_id: u64,
    focal_length: f32,
) -> Option<&'static LensDescription> {
    // LibRaw stores -1 when the maker note has no lens ID.
    if lens_id == u64::MAX {
        return None;
    }
    let make = camera_make.to_ascii_lowercase();
    if make.contains("pentax") || make.contains("ricoh") {
        // LibRaw packs the two LensType bytes as `id << 8 | sub_id`.
        let id = u32::try_from(lens_id >> 8).ok()?;
        pentax((id, (lens_id & 0xff) as u32), focal_length)
    } else if make.contains("olympus") || make.starts_with("om ") {
        // LibRaw packs LensType bytes 0, 2 and 3 as Rawler's composite ID.
        let olympus_id = format!(
            "{:02X} {:02X} {:02X}",
            (lens_id >> 16) & 0xff,
            (lens_id >> 8) & 0xff,
            lens_id & 0xff
        );
        unique_name(
            get_lenses().iter().filter(|lens| {
                lens.mount == MFT_MOUNT
                    && lens.identifiers.olympus_id.as_deref() == Some(olympus_id.as_str())
            }),
            focal_length,
        )
    } else {
        None
    }
}

/// The lens a Pentax DNG names in its `DNGPrivateData`: Pentax writes its
/// maker note there as `PENTAX \0`, a byte order mark and an IFD whose
/// offsets count from the start of the block.
pub(super) fn from_pentax_dng_private_data(
    data: &[u8],
    focal_length: f32,
) -> Option<&'static LensDescription> {
    let body = data.strip_prefix(b"PENTAX \0")?;
    let little_endian = match body.get(..2)? {
        b"II" => true,
        b"MM" => false,
        _ => return None,
    };
    let u16_at = |offset: usize| {
        let bytes: [u8; 2] = data.get(offset..offset + 2)?.try_into().ok()?;
        Some(if little_endian {
            u16::from_le_bytes(bytes)
        } else {
            u16::from_be_bytes(bytes)
        })
    };
    const IFD_START: usize = 10;
    const LENS_TYPE_TAG: u16 = 0x003f;
    const BYTE_TYPE: u16 = 1;
    let entries = usize::from(u16_at(IFD_START)?);
    (0..entries).find_map(|index| {
        let entry = IFD_START + 2 + index * 12;
        if u16_at(entry)? != LENS_TYPE_TAG || u16_at(entry + 2)? != BYTE_TYPE {
            return None;
        }
        // LensType holds 2 to 4 bytes, so they sit inline in the value field.
        let value = data.get(entry + 8..entry + 10)?;
        pentax((u32::from(value[0]), u32::from(value[1])), focal_length)
    })
}

/// The K-mount lens of a Pentax LensType `(id, sub_id)`. IDs 0–2 mean no lens,
/// or a K, M or A lens without electronic identification.
fn pentax(id: (u32, u32), focal_length: f32) -> Option<&'static LensDescription> {
    if id.0 <= 2 {
        return None;
    }
    unique_name(
        get_lenses()
            .iter()
            .filter(|lens| lens.mount == PENTAX_MOUNT && lens.identifiers.id == Some(id)),
        focal_length,
    )
}

/// The one lens among `lenses`, by name. Third-party lenses can share an ID, so
/// when names differ only those covering `focal_length` are kept; a tie left
/// after that names nothing.
fn unique_name(
    lenses: impl Iterator<Item = &'static LensDescription>,
    focal_length: f32,
) -> Option<&'static LensDescription> {
    let lenses = lenses.collect::<Vec<_>>();
    let single = |lenses: &[&'static LensDescription]| {
        let first = *lenses.first()?;
        lenses
            .iter()
            .all(|lens| lens.lens_name == first.lens_name)
            .then_some(first)
    };
    single(&lenses).or_else(|| {
        let covering = lenses
            .iter()
            .copied()
            .filter(|lens| {
                let [low, high] = lens.focal_range;
                low.d != 0
                    && high.d != 0
                    && focal_length >= low.n as f32 / low.d as f32 - 0.5
                    && focal_length <= high.n as f32 / high.d as f32 + 0.5
            })
            .collect::<Vec<_>>();
        single(&covering)
    })
}
