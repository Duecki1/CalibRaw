//! The XMP packet embedded in a photo file: the APP1 segment of a JPEG, or the
//! XMP field (tag 700) in the first IFD of a TIFF-based file such as a TIFF,
//! a DNG or a TIFF-based RAW. The format is sniffed from the first bytes, so
//! only the header region of the file is read.
//!
//! Other containers (PNG, HEIF, CR3, RAF) are not searched. Extended XMP, the
//! continuation of a JPEG packet over 64 KiB, holds no rating and is ignored.

use std::fs::File;
use std::io::{self, BufReader, Read, Seek, SeekFrom};
use std::path::Path;

/// What precedes the packet in a JPEG APP1 segment.
const JPEG_XMP_SIGNATURE: &[u8] = b"http://ns.adobe.com/xap/1.0/\0";
const TIFF_XMP_TAG: u16 = 700;
/// IFDs of real files have a few dozen entries; more is a corrupt file.
const MAX_IFD_ENTRIES: u16 = 4096;

/// The packet embedded in the file at `path`; `None` when it has none or is
/// not a JPEG or TIFF-based file. A truncated file reads as having none.
pub(super) fn embedded_packet(path: &Path, max_bytes: u64) -> io::Result<Option<Vec<u8>>> {
    let mut reader = BufReader::new(File::open(path)?);
    let mut magic = [0_u8; 4];
    let packet = match reader.read_exact(&mut magic) {
        Ok(()) => match magic {
            [0xFF, 0xD8, ..] => {
                reader.seek(SeekFrom::Start(2))?;
                jpeg_packet(&mut reader, max_bytes)
            }
            [b'I', b'I', 42, 0] => tiff_packet(&mut reader, ByteOrder::Little, max_bytes),
            [b'M', b'M', 0, 42] => tiff_packet(&mut reader, ByteOrder::Big, max_bytes),
            _ => Ok(None),
        },
        Err(error) => Err(error),
    };
    match packet {
        Err(error) if error.kind() == io::ErrorKind::UnexpectedEof => Ok(None),
        packet => packet,
    }
}

/// Walks the segments before the scan data, starting after SOI.
fn jpeg_packet<R: Read + Seek>(reader: &mut R, max_bytes: u64) -> io::Result<Option<Vec<u8>>> {
    loop {
        let mut marker = [0_u8; 2];
        reader.read_exact(&mut marker)?;
        if marker[0] != 0xFF {
            return Ok(None);
        }
        // Fill bytes may pad a marker.
        let mut code = marker[1];
        while code == 0xFF {
            code = read_u8(reader)?;
        }
        match code {
            // Markers without a length.
            0x01 | 0xD0..=0xD8 => continue,
            // The scan data or the end: no packet follows.
            0xD9 | 0xDA => return Ok(None),
            _ => {}
        }
        let length = u64::from(u16::from_be_bytes([read_u8(reader)?, read_u8(reader)?]));
        // The length counts its own two bytes.
        let Some(payload) = length.checked_sub(2) else {
            return Ok(None);
        };
        let signature = JPEG_XMP_SIGNATURE.len() as u64;
        if code == 0xE1 && payload >= signature {
            let mut found = vec![0_u8; JPEG_XMP_SIGNATURE.len()];
            reader.read_exact(&mut found)?;
            let rest = payload - signature;
            if found == JPEG_XMP_SIGNATURE {
                return read_bounded(reader, rest, max_bytes).map(Some);
            }
            skip(reader, rest)?;
        } else {
            skip(reader, payload)?;
        }
    }
}

#[derive(Clone, Copy)]
enum ByteOrder {
    Little,
    Big,
}

impl ByteOrder {
    fn u16(self, bytes: [u8; 2]) -> u16 {
        match self {
            Self::Little => u16::from_le_bytes(bytes),
            Self::Big => u16::from_be_bytes(bytes),
        }
    }

    fn u32(self, bytes: [u8; 4]) -> u32 {
        match self {
            Self::Little => u32::from_le_bytes(bytes),
            Self::Big => u32::from_be_bytes(bytes),
        }
    }
}

/// Finds the XMP field in the first IFD, after the 4-byte header was read.
fn tiff_packet<R: Read + Seek>(
    reader: &mut R,
    order: ByteOrder,
    max_bytes: u64,
) -> io::Result<Option<Vec<u8>>> {
    let mut word = [0_u8; 4];
    reader.read_exact(&mut word)?;
    reader.seek(SeekFrom::Start(u64::from(order.u32(word))))?;
    let mut count = [0_u8; 2];
    reader.read_exact(&mut count)?;
    let count = order.u16(count);
    if count > MAX_IFD_ENTRIES {
        return Ok(None);
    }
    let mut entries = vec![0_u8; usize::from(count) * 12];
    reader.read_exact(&mut entries)?;
    for entry in entries.chunks_exact(12) {
        let tag = order.u16([entry[0], entry[1]]);
        let field_type = order.u16([entry[2], entry[3]]);
        // BYTE or UNDEFINED, one byte per value.
        if tag != TIFF_XMP_TAG || !matches!(field_type, 1 | 7) {
            continue;
        }
        let length = u64::from(order.u32([entry[4], entry[5], entry[6], entry[7]]));
        let value = [entry[8], entry[9], entry[10], entry[11]];
        if length <= 4 {
            // A packet that short holds nothing, but it is stored in place.
            return Ok(Some(value[..length as usize].to_vec()));
        }
        reader.seek(SeekFrom::Start(u64::from(order.u32(value))))?;
        return read_bounded(reader, length, max_bytes).map(Some);
    }
    Ok(None)
}

fn read_u8<R: Read>(reader: &mut R) -> io::Result<u8> {
    let mut byte = [0_u8; 1];
    reader.read_exact(&mut byte)?;
    Ok(byte[0])
}

fn skip<R: Seek>(reader: &mut R, bytes: u64) -> io::Result<()> {
    let bytes = i64::try_from(bytes).map_err(|_| io::ErrorKind::InvalidData)?;
    reader.seek(SeekFrom::Current(bytes)).map(drop)
}

fn read_bounded<R: Read>(reader: &mut R, length: u64, max_bytes: u64) -> io::Result<Vec<u8>> {
    if length > max_bytes {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("the embedded XMP is {length} bytes; the limit is {max_bytes} bytes"),
        ));
    }
    let mut bytes = Vec::new();
    reader.take(length).read_to_end(&mut bytes)?;
    if (bytes.len() as u64) < length {
        return Err(io::ErrorKind::UnexpectedEof.into());
    }
    Ok(bytes)
}
