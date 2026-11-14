//! `CompactPixelMap`: a sparse per-pixel map stored as sorted runs.

use super::*;

#[derive(Clone, Debug)]
pub struct CompactPixelMap<T> {
    pub(super) width: u32,
    pub(super) height: u32,
    storage_width: u32,
    storage_height: u32,
    pub(super) values: Vec<T>,
}

impl<T> CompactPixelMap<T> {
    pub fn dense(width: u32, height: u32, values: Vec<T>) -> Self {
        debug_assert_eq!(
            values.len(),
            (width as usize).saturating_mul(height as usize)
        );
        Self {
            width,
            height,
            storage_width: width,
            storage_height: height,
            values,
        }
    }

    pub fn repeating(
        width: u32,
        height: u32,
        storage_width: u32,
        storage_height: u32,
        values: Vec<T>,
    ) -> Self {
        debug_assert!(storage_width > 0 && storage_height > 0);
        debug_assert_eq!(
            values.len(),
            (storage_width as usize).saturating_mul(storage_height as usize)
        );
        Self {
            width,
            height,
            storage_width,
            storage_height,
            values,
        }
    }

    pub fn len(&self) -> usize {
        (self.width as usize).saturating_mul(self.height as usize)
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
    pub fn storage_width(&self) -> u32 {
        self.storage_width
    }
    pub fn storage_height(&self) -> u32 {
        self.storage_height
    }
    pub fn storage_slice(&self) -> &[T] {
        &self.values
    }

    fn storage_index(&self, index: usize) -> usize {
        // Dense maps and uniform black levels are common in the full-resolution
        // processing loops. Neither needs the divisions used for a CFA tile.
        if self.storage_width == self.width && self.storage_height == self.height {
            return index;
        }
        if self.values.len() == 1 {
            return 0;
        }
        let width = self.width.max(1) as usize;
        let x = index % width;
        let y = index / width;
        (y % self.storage_height.max(1) as usize) * self.storage_width.max(1) as usize
            + (x % self.storage_width.max(1) as usize)
    }

    pub fn get(&self, index: usize) -> Option<&T> {
        (index < self.len()).then(|| &self.values[self.storage_index(index)])
    }

    pub fn iter(&self) -> CompactPixelMapIter<'_, T> {
        CompactPixelMapIter { map: self, next: 0 }
    }

    pub fn storage_parts(&self) -> (u32, u32, &[T]) {
        (self.storage_width, self.storage_height, &self.values)
    }
}

impl<T: Copy> CompactPixelMap<T> {
    pub fn append_row_to(&self, y: u32, output: &mut Vec<T>) {
        if y >= self.height || self.width == 0 || self.values.is_empty() {
            return;
        }
        let storage_width = self.storage_width.max(1) as usize;
        let storage_y = (y % self.storage_height.max(1)) as usize;
        let start = storage_y * storage_width;
        let width = self.width as usize;
        let pattern = &self.values[start..start + storage_width.min(width)];
        // The row repeats `pattern`; copying what is written so far doubles it
        // in a few copies where a short CFA pattern would take one per period.
        // Every copy starts at the row's start and its length is a whole number
        // of periods, so the phase carries on.
        let row_start = output.len();
        output.extend_from_slice(pattern);
        while output.len() - row_start < width {
            let written = output.len() - row_start;
            let copied = written.min(width - written);
            output.extend_from_within(row_start..row_start + copied);
        }
    }
}

impl<T: Copy + PartialEq> CompactPixelMap<T> {
    pub fn compact_from_dense(width: u32, height: u32, values: Vec<T>, max_period: u32) -> Self {
        if width == 0 || height == 0 || values.is_empty() {
            return Self::dense(width, height, values);
        }
        if values.len() > 4_000_000 {
            return Self::dense(width, height, values);
        }
        let candidates = [1u32, 2, 3, 4, 6, 8, 12, 16, 24, 32, 48, 64];
        for ph in candidates
            .into_iter()
            .filter(|p| *p <= height && *p <= max_period.max(1))
        {
            for pw in candidates
                .into_iter()
                .filter(|p| *p <= width && *p <= max_period.max(1))
            {
                let mut matches = true;
                'outer: for y in 0..height {
                    for x in 0..width {
                        let a = values[(y * width + x) as usize];
                        let b = values[((y % ph) * width + (x % pw)) as usize];
                        if a != b {
                            matches = false;
                            break 'outer;
                        }
                    }
                }
                if matches {
                    let mut pattern = Vec::with_capacity((pw * ph) as usize);
                    for y in 0..ph {
                        pattern.extend_from_slice(
                            &values[(y * width) as usize..(y * width + pw) as usize],
                        );
                    }
                    return Self::repeating(width, height, pw, ph, pattern);
                }
            }
        }
        Self::dense(width, height, values)
    }

    /// The `width` by `height` region at the origin. Positions outside the map
    /// take their [`reflect_index`] mirror, as the samples of a padded tile do.
    ///
    /// A repeating map gives a repeating region when the mirrored positions
    /// keep the storage period, as inside the map, for a uniform map and for a
    /// Bayer mosaic's even period; otherwise the region is dense. Either way
    /// every position holds the value the per-pixel lookup would give.
    pub fn subregion_reflected(
        &self,
        origin_x: i64,
        origin_y: i64,
        width: u32,
        height: u32,
    ) -> Self {
        if width == 0 || height == 0 {
            return Self::dense(width, height, Vec::new());
        }
        let storage_width = self.storage_width.max(1) as usize;
        let storage_height = self.storage_height.max(1) as usize;
        // The storage column and row of every region column and row; the value
        // at (x, y) is `values[rows[y] * storage_width + columns[x]]`, which is
        // what `Index` gives for the mirrored position, dense or repeating.
        let columns: Vec<usize> = (0..width)
            .map(|x| {
                reflect_index(origin_x + i64::from(x), self.width.max(1) as usize) % storage_width
            })
            .collect();
        let rows: Vec<usize> = (0..height)
            .map(|y| {
                reflect_index(origin_y + i64::from(y), self.height.max(1) as usize) % storage_height
            })
            .collect();

        let repeating = self.storage_width < self.width || self.storage_height < self.height;
        if repeating {
            let pattern_width = storage_width.min(width.max(1) as usize);
            let pattern_height = storage_height.min(height.max(1) as usize);
            if has_period(&columns, pattern_width) && has_period(&rows, pattern_height) {
                let mut pattern = Vec::with_capacity(pattern_width * pattern_height);
                for &row in rows.iter().take(pattern_height) {
                    let storage_row = &self.values[row * storage_width..][..storage_width];
                    pattern.extend(columns[..pattern_width].iter().map(|&x| storage_row[x]));
                }
                return Self::repeating(
                    width,
                    height,
                    pattern_width as u32,
                    pattern_height as u32,
                    pattern,
                );
            }
        }

        // Rows that read the same storage row are equal, so each storage row
        // is gathered once and copied for the others.
        let region_width = width as usize;
        let mut values = Vec::with_capacity(region_width.saturating_mul(height as usize));
        let mut first_use = vec![usize::MAX; storage_height];
        for (y, &row) in rows.iter().enumerate() {
            let earlier = first_use[row];
            if earlier != usize::MAX {
                let start = earlier * region_width;
                values.extend_from_within(start..start + region_width);
                continue;
            }
            first_use[row] = y;
            let storage_row = &self.values[row * storage_width..][..storage_width];
            values.extend(columns.iter().map(|&x| storage_row[x]));
        }
        Self::dense(width, height, values)
    }
}

/// Whether `indices` repeats every `period` entries.
fn has_period(indices: &[usize], period: usize) -> bool {
    indices
        .iter()
        .zip(indices.iter().skip(period))
        .all(|(first, later)| first == later)
}

/// The position inside `0..length` that `index` mirrors to, reflecting about
/// the first and last positions without repeating them. An even offset from
/// the edge keeps a Bayer mosaic's colour phase, so a mirrored mosaic stays a
/// valid one.
pub fn reflect_index(index: i64, length: usize) -> usize {
    if length <= 1 {
        return 0;
    }
    let period = (length as i64 - 1) * 2;
    let folded = index.rem_euclid(period);
    if folded < length as i64 {
        folded as usize
    } else {
        (period - folded) as usize
    }
}

impl<T> Index<usize> for CompactPixelMap<T> {
    type Output = T;
    fn index(&self, index: usize) -> &Self::Output {
        assert!(index < self.len(), "compact pixel-map index out of bounds");
        &self.values[self.storage_index(index)]
    }
}

pub struct CompactPixelMapIter<'a, T> {
    map: &'a CompactPixelMap<T>,
    next: usize,
}

impl<'a, T> Iterator for CompactPixelMapIter<'a, T> {
    type Item = &'a T;
    fn next(&mut self) -> Option<Self::Item> {
        let index = self.next;
        if index >= self.map.len() {
            return None;
        }
        self.next += 1;
        Some(&self.map[index])
    }
    fn size_hint(&self) -> (usize, Option<usize>) {
        let remaining = self.map.len().saturating_sub(self.next);
        (remaining, Some(remaining))
    }
}

impl<'a, T> ExactSizeIterator for CompactPixelMapIter<'a, T> {}

impl<'a, T> IntoIterator for &'a CompactPixelMap<T> {
    type Item = &'a T;
    type IntoIter = CompactPixelMapIter<'a, T>;
    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}
