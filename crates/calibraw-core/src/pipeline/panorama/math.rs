//! Small dense linear algebra for the panorama solver: 3-vectors, rotations, symmetric
//! eigen decomposition and linear solves. Everything is `f64`; the systems are tiny
//! (at most a few hundred unknowns), so clarity wins over speed.

pub(super) type Vec3 = [f64; 3];
pub(super) type Mat3 = [[f64; 3]; 3];

pub(super) const IDENTITY: Mat3 = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];

pub(super) fn mul(a: &Mat3, b: &Mat3) -> Mat3 {
    std::array::from_fn(|r| std::array::from_fn(|c| (0..3).map(|k| a[r][k] * b[k][c]).sum()))
}

pub(super) fn transpose(a: &Mat3) -> Mat3 {
    std::array::from_fn(|r| std::array::from_fn(|c| a[c][r]))
}

pub(super) fn apply(m: &Mat3, v: Vec3) -> Vec3 {
    std::array::from_fn(|r| m[r][0] * v[0] + m[r][1] * v[1] + m[r][2] * v[2])
}

pub(super) fn dot(a: Vec3, b: Vec3) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

pub(super) fn cross(a: Vec3, b: Vec3) -> Vec3 {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

pub(super) fn norm(a: Vec3) -> f64 {
    dot(a, a).sqrt()
}

/// `a / |a|`; the zero vector stays zero.
pub(super) fn normalize(a: Vec3) -> Vec3 {
    let n = norm(a);
    if n > 0.0 {
        a.map(|v| v / n)
    } else {
        a
    }
}

pub(super) fn column(m: &Mat3, c: usize) -> Vec3 {
    [m[0][c], m[1][c], m[2][c]]
}

pub(super) fn invert(m: &Mat3) -> Option<Mat3> {
    let cofactor = |r: usize, c: usize| {
        let (r0, r1) = ((r + 1) % 3, (r + 2) % 3);
        let (c0, c1) = ((c + 1) % 3, (c + 2) % 3);
        m[r0][c0] * m[r1][c1] - m[r0][c1] * m[r1][c0]
    };
    let det = (0..3).map(|c| m[0][c] * cofactor(0, c)).sum::<f64>();
    if det.abs() < 1e-300 || !det.is_finite() {
        return None;
    }
    Some(std::array::from_fn(|r| {
        std::array::from_fn(|c| cofactor(c, r) / det)
    }))
}

/// The rotation by angle `|w|` about axis `w / |w|` (Rodrigues' formula).
pub(super) fn exp_rotation(w: Vec3) -> Mat3 {
    let theta = norm(w);
    if theta < 1e-12 {
        // First order: I + [w]×.
        return [[1.0, -w[2], w[1]], [w[2], 1.0, -w[0]], [-w[1], w[0], 1.0]];
    }
    let k = w.map(|v| v / theta);
    let (sin, cos) = theta.sin_cos();
    let t = 1.0 - cos;
    [
        [
            cos + k[0] * k[0] * t,
            k[0] * k[1] * t - k[2] * sin,
            k[0] * k[2] * t + k[1] * sin,
        ],
        [
            k[1] * k[0] * t + k[2] * sin,
            cos + k[1] * k[1] * t,
            k[1] * k[2] * t - k[0] * sin,
        ],
        [
            k[2] * k[0] * t - k[1] * sin,
            k[2] * k[1] * t + k[0] * sin,
            cos + k[2] * k[2] * t,
        ],
    ]
}

/// The rotation `R` maximizing `Σ bᵀ·R·a` for the correlation `s = Σ a·bᵀ` (`s[p][q] =
/// Σ a_p b_q`): the rotation that best takes vectors `a` onto `b`. Horn's closed form
/// ("Closed-form solution of absolute orientation using unit quaternions", 1987) as the
/// eigenvector of the largest eigenvalue of a symmetric 4×4 matrix, which is always a
/// proper rotation. With `s = mᵀ` it is the rotation nearest to `m`.
pub(super) fn best_rotation(s: &Mat3) -> Mat3 {
    let [[sxx, sxy, sxz], [syx, syy, syz], [szx, szy, szz]] = *s;
    let n = [
        [sxx + syy + szz, syz - szy, szx - sxz, sxy - syx],
        [syz - szy, sxx - syy - szz, sxy + syx, szx + sxz],
        [szx - sxz, sxy + syx, -sxx + syy - szz, syz + szy],
        [sxy - syx, szx + sxz, syz + szy, -sxx - syy + szz],
    ];
    let (values, vectors) = symmetric_eigen(n);
    let best = (0..4)
        .max_by(|&a, &b| values[a].total_cmp(&values[b]))
        .unwrap_or(0);
    let q: [f64; 4] = std::array::from_fn(|i| vectors[i][best]);
    quaternion_matrix(q)
}

/// The rotation of unit quaternion `(w, x, y, z)`.
fn quaternion_matrix(q: [f64; 4]) -> Mat3 {
    let length = q.iter().map(|v| v * v).sum::<f64>().sqrt();
    let [w, x, y, z] = q.map(|v| v / length);
    [
        [
            w * w + x * x - y * y - z * z,
            2.0 * (x * y - w * z),
            2.0 * (x * z + w * y),
        ],
        [
            2.0 * (x * y + w * z),
            w * w - x * x + y * y - z * z,
            2.0 * (y * z - w * x),
        ],
        [
            2.0 * (x * z - w * y),
            2.0 * (y * z + w * x),
            w * w - x * x - y * y + z * z,
        ],
    ]
}

/// Eigenvalues and eigenvectors (the columns of the second result) of the symmetric
/// matrix `a`, by cyclic Jacobi rotations.
pub(super) fn symmetric_eigen<const N: usize>(mut a: [[f64; N]; N]) -> ([f64; N], [[f64; N]; N]) {
    let mut v = [[0.0; N]; N];
    for (i, row) in v.iter_mut().enumerate() {
        row[i] = 1.0;
    }
    for _ in 0..64 {
        let off: f64 = (0..N)
            .flat_map(|p| (p + 1..N).map(move |q| (p, q)))
            .map(|(p, q)| a[p][q] * a[p][q])
            .sum();
        let scale: f64 = (0..N).map(|i| a[i][i] * a[i][i]).sum::<f64>() + off;
        if off <= 1e-30 * scale.max(1e-300) {
            break;
        }
        for p in 0..N {
            for q in p + 1..N {
                if a[p][q].abs() < 1e-300 {
                    continue;
                }
                let theta = (a[q][q] - a[p][p]) / (2.0 * a[p][q]);
                let t = theta.signum() / (theta.abs() + (theta * theta + 1.0).sqrt());
                let t = if theta == 0.0 { 1.0 } else { t };
                let c = 1.0 / (t * t + 1.0).sqrt();
                let s = t * c;
                for row in a.iter_mut() {
                    let (akp, akq) = (row[p], row[q]);
                    row[p] = c * akp - s * akq;
                    row[q] = s * akp + c * akq;
                }
                let (row_p, row_q) = (a[p], a[q]);
                a[p] = std::array::from_fn(|k| c * row_p[k] - s * row_q[k]);
                a[q] = std::array::from_fn(|k| s * row_p[k] + c * row_q[k]);
                for row in v.iter_mut() {
                    let (vkp, vkq) = (row[p], row[q]);
                    row[p] = c * vkp - s * vkq;
                    row[q] = s * vkp + c * vkq;
                }
            }
        }
    }
    (std::array::from_fn(|i| a[i][i]), v)
}

/// Solves the symmetric positive definite system `a·x = b` (`a` row-major, `n × n`) by
/// Cholesky decomposition; `None` when `a` is not positive definite.
pub(super) fn solve_positive_definite(a: &[f64], b: &[f64]) -> Option<Vec<f64>> {
    let n = b.len();
    debug_assert_eq!(a.len(), n * n);
    let mut l = vec![0.0; n * n];
    for i in 0..n {
        for j in 0..=i {
            let sum: f64 = a[i * n + j] - (0..j).map(|k| l[i * n + k] * l[j * n + k]).sum::<f64>();
            if i == j {
                if sum <= 0.0 || !sum.is_finite() {
                    return None;
                }
                l[i * n + i] = sum.sqrt();
            } else {
                l[i * n + j] = sum / l[j * n + j];
            }
        }
    }
    let mut y = vec![0.0; n];
    for i in 0..n {
        y[i] = (b[i] - (0..i).map(|k| l[i * n + k] * y[k]).sum::<f64>()) / l[i * n + i];
    }
    let mut x = vec![0.0; n];
    for i in (0..n).rev() {
        x[i] = (y[i] - (i + 1..n).map(|k| l[k * n + i] * x[k]).sum::<f64>()) / l[i * n + i];
    }
    x.iter().all(|v| v.is_finite()).then_some(x)
}

/// A small deterministic generator (xorshift64*), so RANSAC gives the same result on
/// every run and every machine.
pub(super) struct Random(u64);

impl Random {
    pub(super) fn new(seed: u64) -> Self {
        Self(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1)
    }

    /// Uniform in `0..bound` (`bound > 0`).
    pub(super) fn below(&mut self, bound: usize) -> usize {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        let value = self.0.wrapping_mul(0x2545_F491_4F6C_DD1D);
        ((value >> 11) % bound as u64) as usize
    }
}
