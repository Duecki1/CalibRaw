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
        let pattern = &self.values[start..start + storage_width];
        let mut remaining = self.width as usize;
        while remaining >= pattern.len() {
            output.extend_from_slice(pattern);
            remaining -= pattern.len();
        }
        if remaining > 0 {
            output.extend_from_slice(&pattern[..remaining]);
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

    pub fn subregion_clamped(&self, origin_x: i64, origin_y: i64, width: u32, height: u32) -> Self {
        let source_width = self.width.max(1) as i64;
        let source_height = self.height.max(1) as i64;
        let fully_inside = origin_x >= 0
            && origin_y >= 0
            && origin_x + i64::from(width) <= source_width
            && origin_y + i64::from(height) <= source_height;
        let repeating = self.storage_width < self.width || self.storage_height < self.height;

        if fully_inside && repeating {
            let pattern_width = self.storage_width.min(width.max(1));
            let pattern_height = self.storage_height.min(height.max(1));
            let mut pattern = Vec::with_capacity((pattern_width * pattern_height) as usize);
            for y in 0..pattern_height {
                for x in 0..pattern_width {
                    let source_x = (origin_x + i64::from(x)) as u32;
                    let source_y = (origin_y + i64::from(y)) as u32;
                    pattern.push(self[(source_y * self.width + source_x) as usize]);
                }
            }
            return Self::repeating(width, height, pattern_width, pattern_height, pattern);
        }

        let mut values = Vec::with_capacity((width as usize).saturating_mul(height as usize));
        for y in 0..height {
            let source_y = (origin_y + i64::from(y)).clamp(0, source_height - 1) as u32;
            for x in 0..width {
                let source_x = (origin_x + i64::from(x)).clamp(0, source_width - 1) as u32;
                values.push(self[(source_y * self.width + source_x) as usize]);
            }
        }
        Self::dense(width, height, values)
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
