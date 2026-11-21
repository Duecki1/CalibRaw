//! Camera model and the global solve: orientations from pairwise rotations, refined by
//! bundle adjustment and straightened.
//!
//! Follows OpenCV's rotation model (`modules/stitching/src/motion_estimators.cpp`,
//! BSD-3-Clause): a maximum spanning tree of the verified pairs, walked from its centre,
//! gives initial orientations (`HomographyBasedEstimator`); bundle adjustment minimizes
//! the distance between the two rays of every inlier, scaled by the focal length
//! (`BundleAdjusterRay`); and the horizon is levelled from the cameras' axes
//! (`waveCorrect`). Bundle adjustment uses a Huber loss and, like Hugin's cpclean,
//! drops correspondences that stay far off once it has converged and runs again.

use super::matching::PairMatches;
use super::math::{
    apply, column, cross, dot, exp_rotation, mul, norm, normalize, solve_positive_definite,
    symmetric_eigen, transpose, Mat3, Vec3, IDENTITY,
};
use anyhow::{bail, ensure, Result};
use rayon::prelude::*;

/// Pinhole intrinsics in full-resolution undistorted pixels, principal point at the
/// image centre, with an optional radial distortion for lenses no profile corrected.
///
/// The distortion is PanoTools' `b` term (the one Hugin's `SmartOptimise` fits for an
/// uncalibrated lens): a point at radius `r` on the ideal pinhole image is recorded at
/// `r · (b·r² + 1 − b)`, with radii in units of half the shorter image side.
#[derive(Clone, Copy, Debug)]
pub(super) struct Pinhole {
    pub(super) focal: f64,
    cx: f64,
    cy: f64,
    width: f64,
    height: f64,
    distortion: f64,
}

/// Newton steps inverting the radial distortion.
const UNDISTORT_ITERATIONS: usize = 8;

impl Pinhole {
    pub(super) fn new(width: usize, height: usize, focal: f64) -> Self {
        Self {
            focal,
            cx: 0.5 * (width as f64 - 1.0),
            cy: 0.5 * (height as f64 - 1.0),
            width: width as f64,
            height: height as f64,
            distortion: 0.0,
        }
    }

    fn with_focal(self, focal: f64) -> Self {
        Self { focal, ..self }
    }

    fn with_distortion(self, distortion: f64) -> Self {
        Self { distortion, ..self }
    }

    /// The radius distortion is normalized by.
    fn radius(&self) -> f64 {
        0.5 * self.width.min(self.height)
    }

    /// The unit ray of pixel `p` in camera coordinates (x right, y down, z forward).
    pub(super) fn ray(&self, p: [f64; 2]) -> Vec3 {
        let (mut x, mut y) = (p[0] - self.cx, p[1] - self.cy);
        let b = self.distortion;
        if b != 0.0 {
            // Solve b·r³ + (1 − b)·r = recorded radius for the ideal radius r.
            let recorded = x.hypot(y) / self.radius();
            let mut r = recorded;
            for _ in 0..UNDISTORT_ITERATIONS {
                let slope = 3.0 * b * r * r + 1.0 - b;
                if slope <= 1e-6 {
                    break;
                }
                r -= (b * r * r * r + (1.0 - b) * r - recorded) / slope;
            }
            let scale = b * r * r + 1.0 - b;
            if scale > 1e-6 {
                x /= scale;
                y /= scale;
            }
        }
        normalize([x / self.focal, y / self.focal, 1.0])
    }

    /// The pixel a camera-coordinate direction falls on, when it points forward.
    pub(super) fn pixel(&self, direction: Vec3) -> Option<[f64; 2]> {
        if direction[2] <= 1e-9 {
            return None;
        }
        let x = self.focal * direction[0] / direction[2];
        let y = self.focal * direction[1] / direction[2];
        let b = self.distortion;
        let scale = if b == 0.0 {
            1.0
        } else {
            let r = x.hypot(y) / self.radius();
            // Past the radius where the recorded radius stops growing (b < 0), the
            // polynomial folds back onto the image; no pixel records such directions.
            if 3.0 * b * r * r + 1.0 - b <= 0.0 {
                return None;
            }
            b * r * r + 1.0 - b
        };
        Some([x * scale + self.cx, y * scale + self.cy])
    }

    /// Whether pixel `p` lies on the image (pixel centres at integer coordinates).
    pub(super) fn contains(&self, p: [f64; 2]) -> bool {
        p[0] >= 0.0 && p[1] >= 0.0 && p[0] <= self.width - 1.0 && p[1] <= self.height - 1.0
    }

    /// Distance of `p` from the nearest image edge, in pixels.
    pub(super) fn depth(&self, p: [f64; 2]) -> f64 {
        p[0].min(p[1])
            .min(self.width - 1.0 - p[0])
            .min(self.height - 1.0 - p[1])
    }

    pub(super) fn size(&self) -> (f64, f64) {
        (self.width, self.height)
    }
}

/// A frame's intrinsics and orientation.
#[derive(Clone, Copy, Debug)]
pub(super) struct Camera {
    pub(super) pinhole: Pinhole,
    /// Camera to world coordinates.
    pub(super) rotation: Mat3,
}

impl Camera {
    /// The world direction pixel `p` sees.
    pub(super) fn ray(&self, p: [f64; 2]) -> Vec3 {
        apply(&self.rotation, self.pinhole.ray(p))
    }

    /// The pixel that sees world direction `direction`, if it lies in front.
    pub(super) fn pixel(&self, direction: Vec3) -> Option<[f64; 2]> {
        let local: Vec3 = std::array::from_fn(|r| {
            self.rotation[0][r] * direction[0]
                + self.rotation[1][r] * direction[1]
                + self.rotation[2][r] * direction[2]
        });
        self.pinhole.pixel(local)
    }
}

/// Bundle adjustment iterations per run.
const BA_ITERATIONS: usize = 100;
/// After the first run, correspondences off by more than this many robust standard
/// deviations (and at least `MIN_OUTLIER_PIXELS`) are dropped.
const OUTLIER_SIGMAS: f64 = 3.0;
const MIN_OUTLIER_PIXELS: f64 = 2.0;
/// The Huber loss's threshold in pixels: Hugin's `SmartOptimise` sets the optimizer's
/// `huberSigma` to 2. Correspondences on near objects, which parallax shifts by tens of
/// pixels in a handheld panorama, then barely pull the frames away from the distant
/// scene most correspondences agree on.
const HUBER_PIXELS: f64 = 2.0;
/// Hugin discards a fitted distortion of larger magnitude as implausible and solves
/// again without it (`SmartOptimise`).
const MAX_DISTORTION: f64 = 0.2;
/// The panorama width from which Hugin refines the field of view (`SmartOptimise`).
const MIN_FOCAL_SPAN_DEGREES: f64 = 150.0;
/// The median residual, relative to the long edge, beyond which the frames do not
/// fit one rotating camera.
const MAX_MEDIAN_RESIDUAL: f64 = 0.01;

/// What the solver knows of the frames' lens.
#[derive(Clone, Copy)]
pub(super) struct Lens {
    /// Every frame has one focal length.
    pub(super) shared_focal: bool,
    /// The initial focal length came from the metadata rather than an estimate.
    pub(super) focal_from_metadata: bool,
    /// A lens profile corrected the frames' distortion (and, with Lensfun's auto-scale,
    /// enlarged them, so the metadata's focal length no longer fits them exactly).
    pub(super) corrected: bool,
}

/// Solves the frames' orientations and the lens. `pinholes` holds each frame's initial
/// intrinsics.
///
/// The lens is refined as Hugin's `SmartOptimise` does: for a lens no profile corrected
/// (sharing one focal length), one radial distortion is fitted for all frames and the
/// focal length from the metadata is kept unless the panorama spans
/// `MIN_FOCAL_SPAN_DEGREES` (too few degrees determine it poorly, and parallax then
/// drags it away); a fitted distortion beyond `MAX_DISTORTION` is discarded. Otherwise
/// the focal length is refined, as OpenCV's `BundleAdjusterRay` does. `labels` name the
/// frames in errors.
pub(super) fn solve(
    pinholes: &[Pinhole],
    lens: Lens,
    pairs: &[PairMatches],
    labels: &[String],
) -> Result<Vec<Camera>> {
    let n = pinholes.len();
    let tree = spanning_tree(n, pairs, labels)?;
    let center = tree_center(n, pairs, &tree);
    let mut cameras: Vec<Camera> = pinholes
        .iter()
        .map(|&pinhole| Camera {
            pinhole,
            rotation: IDENTITY,
        })
        .collect();
    // Breadth first from the centre: a pair's rotation R takes rays of its first
    // camera onto its second, so R_second = R_first · Rᵀ.
    let mut known = vec![false; n];
    known[center] = true;
    let mut queue = std::collections::VecDeque::from([center]);
    while let Some(from) = queue.pop_front() {
        for &pair in &tree {
            let pair = &pairs[pair];
            let (to, rotation) = if pair.first == from && !known[pair.second] {
                (
                    pair.second,
                    mul(&cameras[from].rotation, &transpose(&pair.rotation)),
                )
            } else if pair.second == from && !known[pair.first] {
                (pair.first, mul(&cameras[from].rotation, &pair.rotation))
            } else {
                continue;
            };
            cameras[to].rotation = rotation;
            known[to] = true;
            queue.push_back(to);
        }
    }
    let observations: Vec<Observation> = pairs
        .iter()
        .flat_map(|pair| {
            pair.points.iter().map(|&(a, b)| Observation {
                first: pair.first,
                second: pair.second,
                a,
                b,
            })
        })
        .collect();
    let initial = cameras.clone();
    let fit_distortion = !lens.corrected && lens.shared_focal;
    let free_focal = if lens.shared_focal {
        FocalParameters::Shared
    } else {
        FocalParameters::PerFrame
    };
    let mut focal = if fit_distortion && lens.focal_from_metadata {
        FocalParameters::Fixed
    } else {
        free_focal
    };
    let mut kept = Adjustment::new(n, center, focal, fit_distortion)
        .run_cleaned(&mut cameras, observations.clone());
    if focal == FocalParameters::Fixed && span_degrees(&cameras) >= MIN_FOCAL_SPAN_DEGREES {
        focal = free_focal;
        kept = Adjustment::new(n, center, focal, fit_distortion)
            .run_cleaned(&mut cameras, observations.clone());
    }
    if fit_distortion && cameras[center].pinhole.distortion.abs() > MAX_DISTORTION {
        cameras = initial;
        kept = Adjustment::new(n, center, focal, false).run_cleaned(&mut cameras, observations);
    }
    let observations = kept;
    let residual = median(&residual_norms(&cameras, &observations));
    let long = cameras
        .iter()
        .map(|c| c.pinhole.width.max(c.pinhole.height))
        .fold(0.0, f64::max);
    crate::diagnostics::record(format!(
        "Panorama: {} correspondences, median residual {residual:.2} px, focal {:.1} px, distortion {:.4}",
        observations.len(),
        cameras[center].pinhole.focal,
        cameras[center].pinhole.distortion
    ));
    ensure!(
        residual <= MAX_MEDIAN_RESIDUAL * long,
        "The photos could not be aligned into one panorama; shoot every frame turning the camera from one spot"
    );
    ensure!(
        cameras
            .iter()
            .all(|c| c.pinhole.focal.is_finite() && c.pinhole.focal > 0.0),
        "The panorama alignment did not converge"
    );
    straighten(&mut cameras);
    Ok(cameras)
}

/// A lower bound of the horizontal angle the frames cover, in degrees: the widest angle
/// between two cameras' viewing directions about their mean vertical, plus one frame's
/// horizontal field of view. It reaches any threshold below 180° whenever the true
/// span does, full turns included.
fn span_degrees(cameras: &[Camera]) -> f64 {
    let up = normalize(cameras.iter().fold([0.0; 3], |sum, c| {
        let y = column(&c.rotation, 1);
        [sum[0] + y[0], sum[1] + y[1], sum[2] + y[2]]
    }));
    let headings: Vec<Vec3> = cameras
        .iter()
        .map(|c| {
            let z = column(&c.rotation, 2);
            let along = dot(z, up);
            normalize([
                z[0] - along * up[0],
                z[1] - along * up[1],
                z[2] - along * up[2],
            ])
        })
        .collect();
    let mut widest: f64 = 0.0;
    for (i, a) in headings.iter().enumerate() {
        for b in &headings[i + 1..] {
            widest = widest.max(dot(*a, *b).clamp(-1.0, 1.0).acos());
        }
    }
    let field = cameras
        .iter()
        .map(|c| 2.0 * (0.5 * c.pinhole.width / c.pinhole.focal).atan())
        .fold(0.0, f64::max);
    (widest + field).to_degrees()
}

/// The pairs of a maximum spanning tree weighted by inlier count (Kruskal). Fails, naming
/// the frames, when the frames do not all connect.
fn spanning_tree(n: usize, pairs: &[PairMatches], labels: &[String]) -> Result<Vec<usize>> {
    let mut parent: Vec<usize> = (0..n).collect();
    fn root(parent: &mut [usize], mut i: usize) -> usize {
        while parent[i] != i {
            parent[i] = parent[parent[i]];
            i = parent[i];
        }
        i
    }
    let mut order: Vec<usize> = (0..pairs.len()).collect();
    order.sort_by_key(|&k| (std::cmp::Reverse(pairs[k].points.len()), k));
    let mut tree = Vec::new();
    for k in order {
        let (a, b) = (
            root(&mut parent, pairs[k].first),
            root(&mut parent, pairs[k].second),
        );
        if a != b {
            parent[a] = b;
            tree.push(k);
        }
    }
    if tree.len() + 1 == n {
        return Ok(tree);
    }
    let roots: Vec<usize> = (0..n).map(|i| root(&mut parent, i)).collect();
    let largest = (0..n)
        .max_by_key(|&r| {
            (
                roots.iter().filter(|&&x| x == r).count(),
                std::cmp::Reverse(r),
            )
        })
        .map(|r| roots[r])
        .unwrap_or(0);
    let outside: Vec<&str> = (0..n)
        .filter(|&i| roots[i] != largest)
        .map(|i| labels[i].as_str())
        .collect();
    bail!(
        "Could not find enough overlap to connect {} to the other photos; panorama frames must overlap",
        outside.join(", ")
    )
}

/// The tree node with the smallest eccentricity (the longest path from it); ties go to
/// the frame nearest the middle of the file order.
fn tree_center(n: usize, pairs: &[PairMatches], tree: &[usize]) -> usize {
    let mut adjacency = vec![Vec::new(); n];
    for &k in tree {
        adjacency[pairs[k].first].push(pairs[k].second);
        adjacency[pairs[k].second].push(pairs[k].first);
    }
    (0..n)
        .min_by_key(|&start| {
            let mut distance = vec![usize::MAX; n];
            distance[start] = 0;
            let mut queue = std::collections::VecDeque::from([start]);
            while let Some(node) = queue.pop_front() {
                for &next in &adjacency[node] {
                    if distance[next] == usize::MAX {
                        distance[next] = distance[node] + 1;
                        queue.push_back(next);
                    }
                }
            }
            let eccentricity = distance.into_iter().max().unwrap_or(0);
            (eccentricity, (2 * start).abs_diff(n - 1))
        })
        .unwrap_or(0)
}

/// One inlier correspondence.
#[derive(Clone, Copy)]
struct Observation {
    first: usize,
    second: usize,
    a: [f64; 2],
    b: [f64; 2],
}

/// OpenCV's ray error of an observation: the difference of the two world rays scaled by
/// `sqrt(f₁·f₂)`, about the pixel distance at the image centre.
fn ray_error(cameras: &[Camera], o: &Observation) -> Vec3 {
    let (c1, c2) = (&cameras[o.first], &cameras[o.second]);
    let scale = (c1.pinhole.focal * c2.pinhole.focal).sqrt();
    let (r1, r2) = (c1.ray(o.a), c2.ray(o.b));
    std::array::from_fn(|k| scale * (r1[k] - r2[k]))
}

fn residual_norms(cameras: &[Camera], observations: &[Observation]) -> Vec<f64> {
    observations
        .par_iter()
        .map(|o| norm(ray_error(cameras, o)))
        .collect()
}

fn median(values: &[f64]) -> f64 {
    if values.is_empty() {
        return 0.0;
    }
    let mut sorted = values.to_vec();
    sorted.sort_by(f64::total_cmp);
    sorted[sorted.len() / 2]
}

/// The parameters bundle adjustment refines: a rotation increment per frame except the
/// fixed centre frame, the logarithm of the focal length (`focal`), and optionally one
/// radial distortion all frames share.
struct Adjustment {
    frames: usize,
    fixed: usize,
    focal: FocalParameters,
    distortion: bool,
}

/// How bundle adjustment treats the focal length.
#[derive(Clone, Copy, PartialEq, Eq)]
enum FocalParameters {
    Fixed,
    Shared,
    PerFrame,
}

/// One parameter an observation depends on.
#[derive(Clone, Copy)]
enum Local {
    /// Rotation axis `axis` of the observation's first or second camera.
    Rotation { second: bool, axis: usize },
    /// The focal length of the first and/or second camera.
    Focal { first: bool, second: bool },
    /// The distortion every camera shares.
    Distortion,
}

impl Adjustment {
    fn new(frames: usize, fixed: usize, focal: FocalParameters, distortion: bool) -> Self {
        Self {
            frames,
            fixed,
            focal,
            distortion,
        }
    }

    /// Adjusts `cameras` to `observations`, drops the correspondences still far off and
    /// adjusts again; returns the correspondences kept.
    fn run_cleaned(
        &self,
        cameras: &mut Vec<Camera>,
        observations: Vec<Observation>,
    ) -> Vec<Observation> {
        self.run(cameras, &observations);
        let residuals = residual_norms(cameras, &observations);
        let sigma = 1.4826 * median(&residuals);
        let limit = (OUTLIER_SIGMAS * sigma).max(MIN_OUTLIER_PIXELS);
        let kept: Vec<Observation> = observations
            .into_iter()
            .zip(&residuals)
            .filter(|(_, &r)| r <= limit)
            .map(|(o, _)| o)
            .collect();
        self.run(cameras, &kept);
        kept
    }

    fn rotation_index(&self, frame: usize, axis: usize) -> Option<usize> {
        match frame.cmp(&self.fixed) {
            std::cmp::Ordering::Equal => None,
            std::cmp::Ordering::Less => Some(3 * frame + axis),
            std::cmp::Ordering::Greater => Some(3 * (frame - 1) + axis),
        }
    }

    fn focal_index(&self, frame: usize) -> Option<usize> {
        let first = 3 * (self.frames - 1);
        match self.focal {
            FocalParameters::Fixed => None,
            FocalParameters::Shared => Some(first),
            FocalParameters::PerFrame => Some(first + frame),
        }
    }

    fn focal_parameters(&self) -> usize {
        match self.focal {
            FocalParameters::Fixed => 0,
            FocalParameters::Shared => 1,
            FocalParameters::PerFrame => self.frames,
        }
    }

    fn distortion_index(&self) -> Option<usize> {
        self.distortion
            .then(|| 3 * (self.frames - 1) + self.focal_parameters())
    }

    fn parameters(&self) -> usize {
        3 * (self.frames - 1) + self.focal_parameters() + usize::from(self.distortion)
    }

    /// The parameters observation `o` depends on, with their global indices.
    fn locals(&self, o: &Observation) -> Vec<(usize, Local)> {
        let mut locals = Vec::with_capacity(8);
        for axis in 0..3 {
            if let Some(index) = self.rotation_index(o.first, axis) {
                locals.push((
                    index,
                    Local::Rotation {
                        second: false,
                        axis,
                    },
                ));
            }
            if let Some(index) = self.rotation_index(o.second, axis) {
                locals.push((index, Local::Rotation { second: true, axis }));
            }
        }
        match (self.focal_index(o.first), self.focal_index(o.second)) {
            (Some(fa), Some(fb)) if fa == fb => locals.push((
                fa,
                Local::Focal {
                    first: true,
                    second: true,
                },
            )),
            (Some(fa), Some(fb)) => {
                locals.push((
                    fa,
                    Local::Focal {
                        first: true,
                        second: false,
                    },
                ));
                locals.push((
                    fb,
                    Local::Focal {
                        first: false,
                        second: true,
                    },
                ));
            }
            _ => {}
        }
        if let Some(index) = self.distortion_index() {
            locals.push((index, Local::Distortion));
        }
        locals
    }

    /// Cameras after the parameter step `step`.
    fn stepped(&self, cameras: &[Camera], step: &[f64]) -> Vec<Camera> {
        cameras
            .iter()
            .enumerate()
            .map(|(frame, camera)| {
                let w: Vec3 = std::array::from_fn(|axis| {
                    self.rotation_index(frame, axis).map_or(0.0, |i| step[i])
                });
                let focal =
                    camera.pinhole.focal * self.focal_index(frame).map_or(0.0, |i| step[i]).exp();
                let distortion =
                    camera.pinhole.distortion + self.distortion_index().map_or(0.0, |i| step[i]);
                Camera {
                    pinhole: camera.pinhole.with_focal(focal).with_distortion(distortion),
                    rotation: mul(&exp_rotation(w), &camera.rotation),
                }
            })
            .collect()
    }

    /// The error of `o` with `local` perturbed by `h`.
    fn perturbed(cameras: &[Camera], o: &Observation, local: Local, h: f64) -> Vec3 {
        let (mut c1, mut c2) = (cameras[o.first], cameras[o.second]);
        match local {
            Local::Rotation { second, axis } => {
                let mut w = [0.0; 3];
                w[axis] = h;
                let camera = if second { &mut c2 } else { &mut c1 };
                camera.rotation = mul(&exp_rotation(w), &camera.rotation);
            }
            Local::Focal { first, second } => {
                if first {
                    c1.pinhole = c1.pinhole.with_focal(c1.pinhole.focal * h.exp());
                }
                if second {
                    c2.pinhole = c2.pinhole.with_focal(c2.pinhole.focal * h.exp());
                }
            }
            Local::Distortion => {
                c1.pinhole = c1.pinhole.with_distortion(c1.pinhole.distortion + h);
                c2.pinhole = c2.pinhole.with_distortion(c2.pinhole.distortion + h);
            }
        }
        let scale = (c1.pinhole.focal * c2.pinhole.focal).sqrt();
        let (r1, r2) = (c1.ray(o.a), c2.ray(o.b));
        std::array::from_fn(|k| scale * (r1[k] - r2[k]))
    }

    /// Huber-weighted cost.
    fn cost(cameras: &[Camera], observations: &[Observation], huber: f64) -> f64 {
        observations
            .par_iter()
            .map(|o| huber_cost(norm(ray_error(cameras, o)), huber))
            .sum()
    }

    /// Levenberg–Marquardt on the robust ray error, with forward-difference Jacobians.
    fn run(&self, cameras: &mut Vec<Camera>, observations: &[Observation]) {
        if observations.is_empty() {
            return;
        }
        const H: f64 = 1e-6;
        let p = self.parameters();
        let huber = HUBER_PIXELS;
        let mut cost = Self::cost(cameras, observations, huber);
        let mut lambda = 1e-3;
        for _ in 0..BA_ITERATIONS {
            let (hessian, gradient) = observations
                .par_chunks(1024)
                .map(|chunk| {
                    let mut hessian = vec![0.0; p * p];
                    let mut gradient = vec![0.0; p];
                    for o in chunk {
                        let error = ray_error(cameras, o);
                        let magnitude = norm(error);
                        let weight = if magnitude <= huber {
                            1.0
                        } else {
                            huber / magnitude
                        };
                        let jacobian: Vec<(usize, Vec3)> = self
                            .locals(o)
                            .into_iter()
                            .map(|(index, local)| {
                                let moved = Self::perturbed(cameras, o, local, H);
                                (index, std::array::from_fn(|k| (moved[k] - error[k]) / H))
                            })
                            .collect();
                        for &(i, ji) in &jacobian {
                            gradient[i] += weight * dot(ji, error);
                            for &(j, jj) in &jacobian {
                                hessian[i * p + j] += weight * dot(ji, jj);
                            }
                        }
                    }
                    (hessian, gradient)
                })
                .reduce(
                    || (vec![0.0; p * p], vec![0.0; p]),
                    |(mut ha, mut ga), (hb, gb)| {
                        ha.iter_mut().zip(hb).for_each(|(a, b)| *a += b);
                        ga.iter_mut().zip(gb).for_each(|(a, b)| *a += b);
                        (ha, ga)
                    },
                );
            let mut improved = false;
            while lambda < 1e10 {
                let mut damped = hessian.clone();
                for i in 0..p {
                    damped[i * p + i] += lambda * hessian[i * p + i] + 1e-12;
                }
                let negative: Vec<f64> = gradient.iter().map(|g| -g).collect();
                let Some(step) = solve_positive_definite(&damped, &negative) else {
                    lambda *= 8.0;
                    continue;
                };
                let candidate = self.stepped(cameras, &step);
                let candidate_cost = Self::cost(&candidate, observations, huber);
                if candidate_cost < cost {
                    let decrease = (cost - candidate_cost) / cost.max(1e-300);
                    *cameras = candidate;
                    cost = candidate_cost;
                    lambda = (lambda * 0.25).max(1e-9);
                    improved = decrease > 1e-10
                        && step.iter().map(|v| v.abs()).fold(0.0, f64::max) > 1e-12;
                    break;
                }
                lambda *= 8.0;
            }
            if !improved {
                break;
            }
        }
    }
}

fn huber_cost(magnitude: f64, huber: f64) -> f64 {
    if magnitude <= huber {
        0.5 * magnitude * magnitude
    } else {
        huber * (magnitude - 0.5 * huber)
    }
}

/// Levels the panorama: world y becomes the vertical (pointing down, as image y does)
/// and world z the cameras' mean viewing direction, so the panorama's centre is ahead.
///
/// The vertical is estimated as OpenCV's `waveCorrect` does: for a panorama spanning
/// horizontally (OpenCV's `autoDetectWaveCorrectKind`), the cameras' x axes lie in the
/// horizontal plane, whose normal is the eigenvector of the smallest eigenvalue of
/// `Σ x·xᵀ`; for one spanning vertically, the x axes all point along the horizon.
fn straighten(cameras: &mut [Camera]) {
    let axes = |c: usize| -> Vec3 {
        cameras.iter().fold([0.0; 3], |sum, camera| {
            let v = column(&camera.rotation, c);
            [sum[0] + v[0], sum[1] + v[1], sum[2] + v[2]]
        })
    };
    let (sum_x, sum_y, sum_z) = (axes(0), axes(1), axes(2));
    let spread = |axis: usize| {
        let values: Vec<f64> = cameras
            .iter()
            .map(|c| c.rotation[axis][2] / c.rotation[2][2].abs().max(1e-9))
            .collect();
        values.iter().copied().fold(f64::NEG_INFINITY, f64::max)
            - values.iter().copied().fold(f64::INFINITY, f64::min)
    };
    let vertical_sweep = spread(1) > spread(0);
    let mut down = if vertical_sweep {
        normalize(cross(sum_z, normalize(sum_x)))
    } else {
        let mut moment = [[0.0; 3]; 3];
        for camera in cameras.iter() {
            let x = column(&camera.rotation, 0);
            for r in 0..3 {
                for c in 0..3 {
                    moment[r][c] += x[r] * x[c];
                }
            }
        }
        let (values, vectors) = symmetric_eigen(moment);
        let mut order = [0usize, 1, 2];
        order.sort_by(|&a, &b| values[a].total_cmp(&values[b]));
        // When every x axis is parallel (frames that barely turn), the horizontal plane
        // is undetermined: keep the cameras' own mean vertical.
        if values[order[1]] < 1e-6 * cameras.len() as f64 {
            normalize(sum_y)
        } else {
            column(&vectors, order[0])
        }
    };
    if norm(down) < 0.5 {
        down = normalize(sum_y);
    }
    if dot(down, sum_y) < 0.0 {
        down = down.map(|v| -v);
    }
    let ahead = {
        let along = dot(sum_z, down);
        let flat = [
            sum_z[0] - along * down[0],
            sum_z[1] - along * down[1],
            sum_z[2] - along * down[2],
        ];
        if norm(flat) > 1e-6 {
            normalize(flat)
        } else {
            // Looking straight up or down: keep the reference camera's forward.
            normalize(cross(normalize(sum_x), down))
        }
    };
    let right = cross(down, ahead);
    let world = [right, down, ahead];
    for camera in cameras.iter_mut() {
        camera.rotation = mul(&world, &camera.rotation);
    }
}
