//! DNG opcode lists (DNG specification 1.4, chapter 6), read for the GainMaps
//! of OpcodeList2 (`rawler_loader::gain_map`) and the bad pixel opcodes of
//! OpcodeList1 (`libraw_loader::declared_defects`). A list is a count, then
//! each opcode's ID, DNG version, flags, parameter size and parameters; the
//! specification stores opcode lists big-endian whatever the file's byte
//! order.

use anyhow::{ensure, Result};

/// One opcode of a list, with its parameters still unread.
pub(super) struct Opcode<'a> {
    pub(super) id: u32,
    pub(super) params: &'a [u8],
}

/// The opcodes of the list `bytes`, in order.
pub(super) fn opcodes(bytes: &[u8]) -> Result<Vec<Opcode<'_>>> {
    let mut reader = Reader::new(bytes);
    let count = reader.u32()?;
    let mut opcodes = Vec::new();
    for _ in 0..count {
        let id = reader.u32()?;
        let _version = reader.u32()?;
        let _flags = reader.u32()?;
        let size = reader.u32()? as usize;
        let params = reader.take(size)?;
        opcodes.push(Opcode { id, params });
    }
    Ok(opcodes)
}

/// Big-endian reader over an opcode list or an opcode's parameters.
pub(super) struct Reader<'a> {
    bytes: &'a [u8],
}

impl<'a> Reader<'a> {
    pub(super) fn new(bytes: &'a [u8]) -> Self {
        Self { bytes }
    }

    pub(super) fn take(&mut self, len: usize) -> Result<&'a [u8]> {
        ensure!(len <= self.bytes.len(), "truncated DNG opcode list");
        let (taken, rest) = self.bytes.split_at(len);
        self.bytes = rest;
        Ok(taken)
    }

    fn array<const N: usize>(&mut self) -> Result<[u8; N]> {
        Ok(self.take(N)?.try_into()?)
    }

    pub(super) fn u32(&mut self) -> Result<u32> {
        Ok(u32::from_be_bytes(self.array()?))
    }

    pub(super) fn f32(&mut self) -> Result<f32> {
        Ok(f32::from_be_bytes(self.array()?))
    }

    pub(super) fn f64(&mut self) -> Result<f64> {
        Ok(f64::from_be_bytes(self.array()?))
    }
}
