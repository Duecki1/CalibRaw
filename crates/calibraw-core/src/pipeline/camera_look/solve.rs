//! Dense symmetric positive-definite solves for the camera match fit's
//! normal equations, in `f64`.

/// A symmetric matrix stored densely, row-major.
pub(super) struct SymmetricMatrix {
    size: usize,
    values: Vec<f64>,
}

impl SymmetricMatrix {
    pub(super) fn zeros(size: usize) -> Self {
        Self {
            size,
            values: vec![0.0; size * size],
        }
    }

    /// Adds `value` at `(row, column)` and its mirror.
    pub(super) fn add_symmetric(&mut self, row: usize, column: usize, value: f64) {
        self.values[row * self.size + column] += value;
        if row != column {
            self.values[column * self.size + row] += value;
        }
    }

    /// Adds `value` at `(row, column)` only; callers adding every ordered
    /// pair keep the matrix symmetric.
    pub(super) fn add(&mut self, row: usize, column: usize, value: f64) {
        self.values[row * self.size + column] += value;
    }

    pub(super) fn diagonal(&self, index: usize) -> f64 {
        self.values[index * self.size + index]
    }

    /// Solves `self · x = rhs` for each of the `columns` interleaved
    /// right-hand sides (`rhs[row * columns + column]`) in place, by
    /// Cholesky factorization. `None` when the matrix is not positive
    /// definite.
    pub(super) fn solve(mut self, rhs: &mut [f64], columns: usize) -> Option<()> {
        let n = self.size;
        debug_assert_eq!(rhs.len(), n * columns);
        let a = &mut self.values;
        for j in 0..n {
            let mut diagonal = a[j * n + j];
            for k in 0..j {
                diagonal -= a[j * n + k] * a[j * n + k];
            }
            if !diagonal.is_finite() || diagonal <= 0.0 {
                return None;
            }
            let diagonal = diagonal.sqrt();
            a[j * n + j] = diagonal;
            for i in j + 1..n {
                let mut value = a[i * n + j];
                for k in 0..j {
                    value -= a[i * n + k] * a[j * n + k];
                }
                a[i * n + j] = value / diagonal;
            }
        }
        for column in 0..columns {
            // Forward substitution with the lower factor, then back
            // substitution with its transpose.
            for i in 0..n {
                let mut value = rhs[i * columns + column];
                for k in 0..i {
                    value -= a[i * n + k] * rhs[k * columns + column];
                }
                rhs[i * columns + column] = value / a[i * n + i];
            }
            for i in (0..n).rev() {
                let mut value = rhs[i * columns + column];
                for k in i + 1..n {
                    value -= a[k * n + i] * rhs[k * columns + column];
                }
                rhs[i * columns + column] = value / a[i * n + i];
            }
        }
        Some(())
    }
}
