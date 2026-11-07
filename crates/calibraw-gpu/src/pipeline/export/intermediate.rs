//! Intermediates of an export: the linear raster a geometry export resamples, the raster an
//! encoder compresses and a JPEG XL codestream awaiting its container. Each is held in memory
//! while the export's RAM allowance has room for it, and otherwise written to a staging file
//! beside the output and mapped back.

use super::*;

/// Share of the device's RAM an export may hold its intermediates in: on a 16 GB desktop a
/// 100 MP photo's linear raster (1.2 GB), on a 4 GB tablet a 24 MP photo's (290 MB). Larger
/// intermediates, and all of them where the RAM size is unknown, are staged.
const RAM_SHARE_DIVISOR: u64 = 8;

/// The RAM an export's intermediates may still take.
pub(super) struct IntermediateMemory {
    available: AtomicU64,
}

impl IntermediateMemory {
    /// The allowance of one export on this device.
    pub(super) fn for_this_device() -> Self {
        let bytes = calibraw_core::system_memory::total_memory_bytes()
            .map_or(0, |total| total / RAM_SHARE_DIVISOR);
        Self {
            available: AtomicU64::new(bytes),
        }
    }

    /// Takes `bytes` of the allowance until the reservation drops, if it has
    /// that much left.
    pub(super) fn reserve(&self, bytes: u64) -> Option<MemoryReservation<'_>> {
        self.available
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |available| {
                available.checked_sub(bytes)
            })
            .ok()?;
        Some(MemoryReservation {
            memory: self,
            bytes,
        })
    }
}

/// Part of an [`IntermediateMemory`], returned on drop.
pub(super) struct MemoryReservation<'a> {
    memory: &'a IntermediateMemory,
    bytes: u64,
}

impl Drop for MemoryReservation<'_> {
    fn drop(&mut self) {
        self.memory
            .available
            .fetch_add(self.bytes, Ordering::AcqRel);
    }
}

/// Bytes in memory, stored as words so the linear raster can be read as `f32`.
struct MemoryBytes {
    words: Vec<u32>,
    len: usize,
}

impl MemoryBytes {
    fn as_bytes(&self) -> &[u8] {
        &bytemuck::cast_slice(&self.words)[..self.len]
    }
}

/// Writes an intermediate; [`IntermediateWriter::finish`] gives it back for reading.
pub(super) struct IntermediateWriter<'a> {
    target: WriterTarget<'a>,
}

enum WriterTarget<'a> {
    Memory {
        bytes: MemoryBytes,
        _reservation: MemoryReservation<'a>,
    },
    File {
        writer: BufWriter<fs::File>,
        staged: StagedFile,
    },
}

impl<'a> IntermediateWriter<'a> {
    /// An intermediate of `output` expected to take `len` bytes, held in memory if `memory`
    /// has room for that much and the allocation succeeds. `label` names it in diagnostics.
    pub(super) fn new(
        output: ExportOutput<'_>,
        len: u64,
        memory: &'a IntermediateMemory,
        label: &str,
    ) -> Result<Self> {
        let held = memory.reserve(len).and_then(|reservation| {
            let words = usize::try_from(len.div_ceil(4)).ok()?;
            let mut storage = Vec::new();
            storage.try_reserve_exact(words).ok()?;
            Some((storage, reservation))
        });
        let megabytes = len as f64 / (1024.0 * 1024.0);
        let target = match held {
            Some((words, reservation)) => {
                calibraw_core::diagnostics::record(format!(
                    "Export {label} ({megabytes:.1} MiB) held in memory"
                ));
                WriterTarget::Memory {
                    bytes: MemoryBytes { words, len: 0 },
                    _reservation: reservation,
                }
            }
            None => {
                calibraw_core::diagnostics::record(format!(
                    "Export {label} ({megabytes:.1} MiB) staged beside the output"
                ));
                let staged = StagedFile::for_output(output)?;
                let file = OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(staged.path())
                    .with_context(|| {
                        format!("create staged export {label} {}", staged.path().display())
                    })?;
                WriterTarget::File {
                    writer: BufWriter::new(file),
                    staged,
                }
            }
        };
        Ok(Self { target })
    }

    /// The finished intermediate.
    pub(super) fn finish(self) -> Result<Intermediate<'a>> {
        let held = match self.target {
            WriterTarget::Memory {
                bytes,
                _reservation,
            } => Held::Memory {
                bytes,
                _reservation,
            },
            WriterTarget::File { writer, staged } => {
                let file = writer
                    .into_inner()
                    .map_err(|error| error.into_error())
                    .with_context(|| format!("flush staged export {}", staged.path().display()))?;
                // SAFETY: the staged file has a unique name, is finished above and is not
                // written again; `Held::Mapped` drops the mapping before the file is removed.
                let map = unsafe { memmap2::MmapOptions::new().map(&file) }
                    .with_context(|| format!("map staged export {}", staged.path().display()))?;
                Held::Mapped {
                    map,
                    _staged: staged,
                }
            }
        };
        Ok(Intermediate { held })
    }
}

impl Write for IntermediateWriter<'_> {
    fn write(&mut self, data: &[u8]) -> std::io::Result<usize> {
        match &mut self.target {
            WriterTarget::Memory { bytes, .. } => {
                let end = bytes.len + data.len();
                if end > bytes.words.len() * 4 {
                    // Grows past the reserved capacity only if the expected length was low.
                    bytes.words.resize(end.div_ceil(4), 0);
                }
                bytemuck::cast_slice_mut(&mut bytes.words)[bytes.len..end].copy_from_slice(data);
                bytes.len = end;
                Ok(data.len())
            }
            WriterTarget::File { writer, .. } => writer.write(data),
        }
    }

    fn flush(&mut self) -> std::io::Result<()> {
        match &mut self.target {
            WriterTarget::Memory { .. } => Ok(()),
            WriterTarget::File { writer, .. } => writer.flush(),
        }
    }
}

/// A finished intermediate, read as bytes.
pub(super) struct Intermediate<'a> {
    held: Held<'a>,
}

enum Held<'a> {
    Memory {
        bytes: MemoryBytes,
        _reservation: MemoryReservation<'a>,
    },
    // Fields drop in order: the mapping before the staged file is removed.
    Mapped {
        map: memmap2::Mmap,
        _staged: StagedFile,
    },
}

impl std::ops::Deref for Intermediate<'_> {
    type Target = [u8];

    fn deref(&self) -> &[u8] {
        match &self.held {
            Held::Memory { bytes, .. } => bytes.as_bytes(),
            Held::Mapped { map, .. } => map,
        }
    }
}
