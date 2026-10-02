//! Fixed-size, row-major `f32` matrix helpers for the color pipeline.

pub type Matrix3 = [[f32; 3]; 3];

pub const IDENTITY3: Matrix3 = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];

pub fn multiply<const ROWS: usize, const INNER: usize, const COLUMNS: usize>(
    left: [[f32; INNER]; ROWS],
    right: [[f32; COLUMNS]; INNER],
) -> [[f32; COLUMNS]; ROWS] {
    std::array::from_fn(|row| {
        std::array::from_fn(|column| {
            (0..INNER).fold(0.0, |sum, index| {
                sum + left[row][index] * right[index][column]
            })
        })
    })
}

/// Multiplies `matrix` by the column vector `vector`.
pub fn transform<const ROWS: usize, const COLUMNS: usize>(
    matrix: [[f32; COLUMNS]; ROWS],
    vector: [f32; COLUMNS],
) -> [f32; ROWS] {
    matrix.map(|row| (0..COLUMNS).fold(0.0, |sum, index| sum + row[index] * vector[index]))
}

pub fn lerp<const ROWS: usize, const COLUMNS: usize>(
    start: [[f32; COLUMNS]; ROWS],
    end: [[f32; COLUMNS]; ROWS],
    amount: f32,
) -> [[f32; COLUMNS]; ROWS] {
    std::array::from_fn(|row| {
        std::array::from_fn(|column| {
            start[row][column] + (end[row][column] - start[row][column]) * amount
        })
    })
}

/// Gauss-Jordan inverse with partial pivoting, computed in `f64` so that
/// poorly conditioned camera matrices keep their precision. Returns `None`
/// for singular or non-finite input.
pub fn invert<const N: usize>(matrix: [[f32; N]; N]) -> Option<[[f32; N]; N]> {
    let mut reduced = matrix.map(|row| row.map(f64::from));
    let mut inverse: [[f64; N]; N] =
        std::array::from_fn(|row| std::array::from_fn(|column| f64::from(row == column)));
    for pivot in 0..N {
        let mut best = pivot;
        for row in pivot + 1..N {
            if reduced[row][pivot].abs() > reduced[best][pivot].abs() {
                best = row;
            }
        }
        if !reduced[best][pivot].is_finite() || reduced[best][pivot].abs() < 1e-14 {
            return None;
        }
        reduced.swap(pivot, best);
        inverse.swap(pivot, best);
        let divisor = reduced[pivot][pivot];
        for value in reduced[pivot].iter_mut().chain(&mut inverse[pivot]) {
            *value /= divisor;
        }
        let (pivot_reduced, pivot_inverse) = (reduced[pivot], inverse[pivot]);
        for row in (0..N).filter(|&row| row != pivot) {
            let factor = reduced[row][pivot];
            for column in 0..N {
                reduced[row][column] -= factor * pivot_reduced[column];
                inverse[row][column] -= factor * pivot_inverse[column];
            }
        }
    }
    let inverse = inverse.map(|row| row.map(|value| value as f32));
    inverse
        .iter()
        .flatten()
        .all(|value| value.is_finite())
        .then_some(inverse)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn multiply_handles_rectangular_shapes() {
        let left = [[1.0, 2.0, 3.0], [4.0, 5.0, 6.0]];
        let right = [[7.0, 8.0], [9.0, 10.0], [11.0, 12.0]];
        assert_eq!(multiply(left, right), [[58.0, 64.0], [139.0, 154.0]]);
        assert_eq!(transform(left, [1.0, 0.0, -1.0]), [-2.0, -2.0]);
    }

    #[test]
    fn invert_round_trips_and_rejects_singular_matrices() {
        let matrix = [[2.0, 0.5, 0.0], [0.25, 1.5, 0.1], [0.0, 0.3, 0.9]];
        let product = multiply(matrix, invert(matrix).unwrap());
        for (row, identity_row) in product.iter().zip(IDENTITY3) {
            for (actual, expected) in row.iter().zip(identity_row) {
                assert!((actual - expected).abs() < 1e-6);
            }
        }
        assert!(invert([[1.0, 2.0], [2.0, 4.0]]).is_none());
        assert!(invert([[f32::NAN, 0.0], [0.0, 1.0]]).is_none());
    }

    #[test]
    fn lerp_interpolates_each_entry() {
        assert_eq!(lerp([[0.0, 2.0]], [[4.0, -2.0]], 0.25), [[1.0, 1.0]]);
    }
}
