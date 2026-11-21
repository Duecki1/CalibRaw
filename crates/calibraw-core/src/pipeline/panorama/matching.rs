//! Pairwise matching and geometric verification, after Brown & Lowe ("Automatic
//! Panoramic Image Stitching using Invariant Features", IJCV 2007) as OpenCV's
//! `BestOf2NearestMatcher` implements it (`modules/stitching/src/matchers.cpp`,
//! BSD-3-Clause):
//!
//! - descriptors are matched in both directions with Lowe's ratio test and the two
//!   directions are united;
//! - a pair is accepted when RANSAC finds more inliers than `8 + 0.3 · matches`
//!   (Brown & Lowe's probabilistic verification, OpenCV's `confidence > 1`).
//!
//! The RANSAC model follows cpfind's `rpy` mode: a camera turning about its centre moves
//! every ray by one rotation, which two correspondences determine (Brown, Hartley &
//! Nistér, "Minimal Solutions for Panoramic Stitching", CVPR 2007). That model is tighter
//! than a homography and stays valid for wide views. It needs the focal length; where the
//! metadata does not give it, pairs are verified with a homography instead, as cpfind's
//! `hom` mode does, the focal length is estimated from those homographies
//! ([`estimate_focal`], OpenCV's `estimateFocal`) and each pair's rotation is fitted to
//! its inliers.

use super::camera::Pinhole;
use super::features::Descriptor;
use super::kdtree::KdTree;
use super::math::{apply, best_rotation, dot, invert, mul, symmetric_eigen, Mat3, Random, Vec3};
use rayon::prelude::*;

/// Lowe's ratio test: the nearest descriptor must be this much closer than the second.
const RATIO: f32 = 0.8;
/// Leaf points examined per nearest-neighbour query (cpfind's `kdtreesteps`).
const KD_CHECKS: usize = 200;
/// With more frames than this, each frame is matched only with the frames it shares the
/// most descriptors with, instead of with every frame.
const ALL_PAIRS_LIMIT: usize = 12;
/// Frames each frame is matched with beyond the all-pairs limit (Brown & Lowe: m = 6).
const CANDIDATES_PER_FRAME: usize = 6;
/// Strongest keypoints per frame used to vote for candidate pairs.
const VOTE_KEYPOINTS: usize = 256;
const VOTE_CHECKS: usize = 32;
/// Inlier tolerance in registration-image pixels.
const RANSAC_TOLERANCE: f64 = 4.0;
const RANSAC_CONFIDENCE: f64 = 0.999;
const RANSAC_MAX_ITERATIONS: usize = 2000;
/// Fewest inliers that verify a pair, whatever the match count.
const MIN_INLIERS: usize = 12;

/// A point in one frame and its match in another, in pixels.
type Correspondence = ([f64; 2], [f64; 2]);

/// The keypoints of one frame.
pub(super) struct FrameFeatures {
    /// Full-resolution undistorted pixel positions.
    pub(super) points: Vec<[f64; 2]>,
    pub(super) descriptors: Vec<Descriptor>,
    /// Full-resolution pixels per registration-image pixel.
    pub(super) pixel: f64,
}

/// Verified correspondences between two frames.
pub(super) struct PairMatches {
    pub(super) first: usize,
    pub(super) second: usize,
    /// Inlier positions in the first and second frame (full-resolution undistorted).
    pub(super) points: Vec<Correspondence>,
    /// Rotation taking the first camera's rays onto the second's.
    pub(super) rotation: Mat3,
}

/// Putative matches between every pair worth matching, as keypoint index pairs.
pub(super) struct Putative {
    pub(super) first: usize,
    pub(super) second: usize,
    pub(super) matches: Vec<(usize, usize)>,
}

/// Matches descriptors between the frame pairs worth comparing.
pub(super) fn match_frames(frames: &[FrameFeatures]) -> Vec<Putative> {
    let trees: Vec<KdTree> = frames
        .par_iter()
        .map(|frame| KdTree::new(&frame.descriptors))
        .collect();
    candidate_pairs(frames, &trees)
        .into_par_iter()
        .map(|(first, second)| Putative {
            first,
            second,
            matches: mutual_matches(frames, &trees, first, second),
        })
        .collect()
}

/// Every pair while there are few frames; otherwise each frame with the frames that share
/// the most descriptors with it and with its neighbours in file order.
fn candidate_pairs(frames: &[FrameFeatures], trees: &[KdTree]) -> Vec<(usize, usize)> {
    let n = frames.len();
    let mut pairs = std::collections::BTreeSet::new();
    if n <= ALL_PAIRS_LIMIT {
        for first in 0..n {
            for second in first + 1..n {
                pairs.insert((first, second));
            }
        }
        return pairs.into_iter().collect();
    }
    let votes: Vec<Vec<usize>> = (0..n)
        .into_par_iter()
        .map(|i| {
            (0..n)
                .map(|j| {
                    if i == j {
                        return 0;
                    }
                    frames[i]
                        .descriptors
                        .iter()
                        .take(VOTE_KEYPOINTS)
                        .filter(|query| {
                            trees[j]
                                .nearest_two(query, VOTE_CHECKS)
                                .is_some_and(passes_ratio)
                        })
                        .count()
                })
                .collect()
        })
        .collect();
    for (i, row) in votes.iter().enumerate() {
        let mut ranked: Vec<usize> = (0..n).filter(|&j| j != i).collect();
        ranked.sort_by_key(|&j| (std::cmp::Reverse(row[j] + votes[j][i]), j));
        for &j in ranked.iter().take(CANDIDATES_PER_FRAME) {
            pairs.insert((i.min(j), i.max(j)));
        }
        if i + 1 < n {
            pairs.insert((i, i + 1));
        }
    }
    pairs.into_iter().collect()
}

fn passes_ratio(nearest: super::kdtree::Nearest) -> bool {
    nearest.distance < RATIO * RATIO * nearest.second
}

/// Ratio-test matches from `first` to `second` united with those from `second` to
/// `first`, as OpenCV's `CpuMatcher::match` does.
fn mutual_matches(
    frames: &[FrameFeatures],
    trees: &[KdTree],
    first: usize,
    second: usize,
) -> Vec<(usize, usize)> {
    let forward: Vec<(usize, usize)> = frames[first]
        .descriptors
        .iter()
        .enumerate()
        .filter_map(|(i, query)| {
            trees[second]
                .nearest_two(query, KD_CHECKS)
                .filter(|n| passes_ratio(*n))
                .map(|n| (i, n.index))
        })
        .collect();
    let known: std::collections::HashSet<(usize, usize)> = forward.iter().copied().collect();
    let mut matches = forward;
    for (j, query) in frames[second].descriptors.iter().enumerate() {
        if let Some(n) = trees[first]
            .nearest_two(query, KD_CHECKS)
            .filter(|n| passes_ratio(*n))
        {
            if !known.contains(&(n.index, j)) {
                matches.push((n.index, j));
            }
        }
    }
    matches
}

/// Brown & Lowe's verification: enough inliers for the number of matches.
fn verified(inliers: usize, matches: usize) -> bool {
    inliers >= MIN_INLIERS && inliers as f64 > 8.0 + 0.3 * matches as f64
}

/// Verifies `putative` with the rotation model for cameras `cameras` (one per frame) and
/// returns the pairs that pass.
pub(super) fn verify_rotations(
    frames: &[FrameFeatures],
    cameras: &[Pinhole],
    putative: &[Putative],
) -> Vec<PairMatches> {
    putative
        .par_iter()
        .filter_map(|pair| {
            let (a, b) = (&frames[pair.first], &frames[pair.second]);
            let (ca, cb) = (&cameras[pair.first], &cameras[pair.second]);
            let rays: Vec<(Vec3, Vec3)> = pair
                .matches
                .iter()
                .map(|&(i, j)| (ca.ray(a.points[i]), cb.ray(b.points[j])))
                .collect();
            // Angular tolerance from the pixel tolerance at the image centre.
            let tolerance = RANSAC_TOLERANCE * a.pixel.max(b.pixel) / ca.focal.min(cb.focal);
            let seed = (pair.first as u64) << 32 | pair.second as u64;
            let (rotation, inliers) = ransac_rotation(&rays, tolerance, seed)?;
            verified(inliers.len(), rays.len()).then(|| PairMatches {
                first: pair.first,
                second: pair.second,
                points: inliers
                    .iter()
                    .map(|&k| {
                        let (i, j) = pair.matches[k];
                        (a.points[i], b.points[j])
                    })
                    .collect(),
                rotation,
            })
        })
        .collect()
}

/// RANSAC over rotations taking rays `a` onto rays `b`, refitted on the inliers.
fn ransac_rotation(rays: &[(Vec3, Vec3)], tolerance: f64, seed: u64) -> Option<(Mat3, Vec<usize>)> {
    if rays.len() < MIN_INLIERS {
        return None;
    }
    // |R·a − b|² of unit vectors is 2 − 2·cos θ ≈ θ².
    let limit = tolerance * tolerance;
    let inliers_of = |rotation: &Mat3| -> Vec<usize> {
        rays.iter()
            .enumerate()
            .filter(|(_, (a, b))| {
                let r = apply(rotation, *a);
                let d = [r[0] - b[0], r[1] - b[1], r[2] - b[2]];
                dot(d, d) < limit
            })
            .map(|(k, _)| k)
            .collect()
    };
    let fit = |indices: &[usize]| -> Mat3 {
        let mut s = [[0.0; 3]; 3];
        for &k in indices {
            let (a, b) = rays[k];
            for p in 0..3 {
                for q in 0..3 {
                    s[p][q] += a[p] * b[q];
                }
            }
        }
        best_rotation(&s)
    };
    let mut random = Random::new(seed);
    let mut best: Option<(Mat3, Vec<usize>)> = None;
    let mut iterations = RANSAC_MAX_ITERATIONS;
    let mut iteration = 0;
    while iteration < iterations {
        iteration += 1;
        let i = random.below(rays.len());
        let j = random.below(rays.len());
        // Two nearly parallel rays leave the roll about them undetermined.
        if i == j || dot(rays[i].0, rays[j].0) > (0.25f64.to_radians()).cos() {
            continue;
        }
        let rotation = fit(&[i, j]);
        let inliers = inliers_of(&rotation);
        if best.as_ref().is_none_or(|(_, b)| inliers.len() > b.len()) {
            iterations = iterations.min(required_iterations(inliers.len(), rays.len(), 2));
            best = Some((rotation, inliers));
        }
    }
    let (_, inliers) = best?;
    if inliers.len() < 2 {
        return None;
    }
    let rotation = fit(&inliers);
    let inliers = inliers_of(&rotation);
    Some((rotation, inliers))
}

/// RANSAC iterations that find an all-inlier sample of `sample` points with
/// `RANSAC_CONFIDENCE`, at the observed inlier ratio.
fn required_iterations(inliers: usize, total: usize, sample: i32) -> usize {
    let ratio = inliers as f64 / total as f64;
    let success = ratio.powi(sample);
    if success >= 1.0 {
        return 1;
    }
    if success <= 0.0 {
        return RANSAC_MAX_ITERATIONS;
    }
    let iterations = (1.0 - RANSAC_CONFIDENCE).ln() / (1.0 - success).ln();
    (iterations.ceil() as usize).clamp(1, RANSAC_MAX_ITERATIONS)
}

/// A pair verified by a homography: what frames with an unknown focal length are
/// verified by, as cpfind verifies narrow views, since the rotation model needs the
/// focal length.
pub(super) struct HomographyPair {
    first: usize,
    second: usize,
    /// Inlier positions in the first and second frame (full-resolution undistorted).
    points: Vec<Correspondence>,
    /// The homography between the frames' centred pixel coordinates.
    homography: Mat3,
}

/// Verifies `putative` with homographies between the frames, whose full-resolution
/// sizes are `sizes`, and returns the pairs that pass.
pub(super) fn verify_homographies(
    frames: &[FrameFeatures],
    sizes: &[(usize, usize)],
    putative: &[Putative],
) -> Vec<HomographyPair> {
    let center = |frame: usize, p: [f64; 2]| {
        let (w, h) = sizes[frame];
        [p[0] - 0.5 * (w as f64 - 1.0), p[1] - 0.5 * (h as f64 - 1.0)]
    };
    putative
        .par_iter()
        .filter_map(|pair| {
            let (a, b) = (&frames[pair.first], &frames[pair.second]);
            let centred: Vec<Correspondence> = pair
                .matches
                .iter()
                .map(|&(i, j)| {
                    (
                        center(pair.first, a.points[i]),
                        center(pair.second, b.points[j]),
                    )
                })
                .collect();
            let tolerance = RANSAC_TOLERANCE * a.pixel.max(b.pixel);
            let seed = (pair.first as u64) << 32 | pair.second as u64;
            let (homography, inliers) = ransac_homography(&centred, tolerance, seed)?;
            verified(inliers.len(), centred.len()).then(|| HomographyPair {
                first: pair.first,
                second: pair.second,
                points: inliers
                    .iter()
                    .map(|&k| {
                        let (i, j) = pair.matches[k];
                        (a.points[i], b.points[j])
                    })
                    .collect(),
                homography,
            })
        })
        .collect()
}

/// The focal length (full-resolution pixels) estimated from the pairs' homographies,
/// after OpenCV's `estimateFocal` and `focalsFromHomography` (`modules/stitching/src/
/// autocalib.cpp`, BSD-3-Clause; Szeliski & Shum, "Creating full view panoramic image
/// mosaics and environment maps", 1997): the median over the pairs whose homography
/// gives both focal lengths. `None` with fewer estimates than frames minus one, as
/// OpenCV requires.
pub(super) fn estimate_focal(pairs: &[HomographyPair], frames: usize) -> Option<f64> {
    let mut focals: Vec<f64> = pairs
        .iter()
        .filter_map(|pair| focals_from_homography(&pair.homography))
        .map(|(f0, f1)| (f0 * f1).sqrt())
        .collect();
    if focals.is_empty() || focals.len() + 1 < frames {
        return None;
    }
    focals.sort_by(f64::total_cmp);
    let middle = focals.len() / 2;
    Some(if focals.len() % 2 == 1 {
        focals[middle]
    } else {
        0.5 * (focals[middle - 1] + focals[middle])
    })
}

/// The rotation of every homography-verified pair for cameras `cameras`, fitted to its
/// inliers. Bundle adjustment then refines the focal length the rays were cast with.
pub(super) fn rotations_from_inliers(
    pairs: Vec<HomographyPair>,
    cameras: &[Pinhole],
) -> Vec<PairMatches> {
    pairs
        .into_iter()
        .map(|pair| {
            let (ca, cb) = (&cameras[pair.first], &cameras[pair.second]);
            let mut s = [[0.0; 3]; 3];
            for &(p, q) in &pair.points {
                let (a, b) = (ca.ray(p), cb.ray(q));
                for r in 0..3 {
                    for c in 0..3 {
                        s[r][c] += a[r] * b[c];
                    }
                }
            }
            PairMatches {
                first: pair.first,
                second: pair.second,
                points: pair.points,
                rotation: best_rotation(&s),
            }
        })
        .collect()
}

/// OpenCV's `focalsFromHomography` for a homography between centred pixel coordinates
/// (Copyright (C) 2000-2008, Intel Corporation; Copyright (C) 2009, Willow Garage Inc.;
/// 3-clause BSD, reproduced in `THIRD_PARTY_NOTICES.md`).
fn focals_from_homography(h: &Mat3) -> Option<(f64, f64)> {
    let h = [
        h[0][0], h[0][1], h[0][2], h[1][0], h[1][1], h[1][2], h[2][0], h[2][1], h[2][2],
    ];
    let pick = |d1: f64, d2: f64, v1: f64, v2: f64| -> Option<f64> {
        let (d1, d2, v1, v2) = if v1 < v2 {
            (d2, d1, v2, v1)
        } else {
            (d1, d2, v1, v2)
        };
        if v1 > 0.0 && v2 > 0.0 {
            Some(if d1.abs() > d2.abs() { v1 } else { v2 }.sqrt())
        } else if v1 > 0.0 {
            Some(v1.sqrt())
        } else {
            None
        }
    };
    let d1 = h[6] * h[7];
    let d2 = (h[7] - h[6]) * (h[7] + h[6]);
    let v1 = -(h[0] * h[1] + h[3] * h[4]) / d1;
    let v2 = (h[0] * h[0] + h[3] * h[3] - h[1] * h[1] - h[4] * h[4]) / d2;
    let f1 = pick(d1, d2, v1, v2)?;
    let d1 = h[0] * h[3] + h[1] * h[4];
    let d2 = h[0] * h[0] + h[1] * h[1] - h[3] * h[3] - h[4] * h[4];
    let v1 = -h[2] * h[5] / d1;
    let v2 = (h[5] * h[5] - h[2] * h[2]) / d2;
    let f0 = pick(d1, d2, v1, v2)?;
    (f0.is_finite() && f1.is_finite()).then_some((f0, f1))
}

/// RANSAC over homographies taking `points.0` onto `points.1` (4-point DLT), refitted on
/// the inliers. Returns the homography and the indices of its inliers.
fn ransac_homography(
    points: &[Correspondence],
    tolerance: f64,
    seed: u64,
) -> Option<(Mat3, Vec<usize>)> {
    if points.len() < MIN_INLIERS {
        return None;
    }
    let limit = tolerance * tolerance;
    let inliers_of = |h: &Mat3| -> Vec<usize> {
        points
            .iter()
            .enumerate()
            .filter(|(_, (p, q))| {
                let m = apply(h, [p[0], p[1], 1.0]);
                if m[2].abs() < 1e-12 {
                    return false;
                }
                let (dx, dy) = (m[0] / m[2] - q[0], m[1] / m[2] - q[1]);
                dx * dx + dy * dy < limit
            })
            .map(|(k, _)| k)
            .collect()
    };
    let mut random = Random::new(seed ^ 0x5bd1_e995);
    let mut best: Option<Vec<usize>> = None;
    let mut iterations = RANSAC_MAX_ITERATIONS;
    let mut iteration = 0;
    while iteration < iterations {
        iteration += 1;
        let sample: [usize; 4] = std::array::from_fn(|_| random.below(points.len()));
        if (0..4).any(|a| (a + 1..4).any(|b| sample[a] == sample[b])) {
            continue;
        }
        let Some(h) = fit_homography(points, &sample) else {
            continue;
        };
        let inliers = inliers_of(&h);
        if best.as_ref().is_none_or(|b| inliers.len() > b.len()) {
            iterations = iterations.min(required_iterations(inliers.len(), points.len(), 4));
            best = Some(inliers);
        }
    }
    let inliers = best?;
    let h = fit_homography(points, &inliers)?;
    let inliers = inliers_of(&h);
    Some((h, inliers))
}

/// Least-squares homography through `points[indices]` by the normalized DLT (Hartley &
/// Zisserman, "Multiple View Geometry", algorithm 4.2).
fn fit_homography(points: &[Correspondence], indices: &[usize]) -> Option<Mat3> {
    let normalization = |select: fn(&Correspondence) -> [f64; 2]| -> Option<Mat3> {
        let n = indices.len() as f64;
        let (mut mx, mut my) = (0.0, 0.0);
        for &k in indices {
            let p = select(&points[k]);
            mx += p[0] / n;
            my += p[1] / n;
        }
        let spread = indices
            .iter()
            .map(|&k| {
                let p = select(&points[k]);
                (p[0] - mx).hypot(p[1] - my)
            })
            .sum::<f64>()
            / n;
        if spread < 1e-9 {
            return None;
        }
        let s = std::f64::consts::SQRT_2 / spread;
        Some([[s, 0.0, -s * mx], [0.0, s, -s * my], [0.0, 0.0, 1.0]])
    };
    let t1 = normalization(|p| p.0)?;
    let t2 = normalization(|p| p.1)?;
    let mut ata = [[0.0f64; 9]; 9];
    for &k in indices {
        let (p, q) = points[k];
        let a = apply(&t1, [p[0], p[1], 1.0]);
        let b = apply(&t2, [q[0], q[1], 1.0]);
        let rows = [
            [
                -a[0],
                -a[1],
                -1.0,
                0.0,
                0.0,
                0.0,
                b[0] * a[0],
                b[0] * a[1],
                b[0],
            ],
            [
                0.0,
                0.0,
                0.0,
                -a[0],
                -a[1],
                -1.0,
                b[1] * a[0],
                b[1] * a[1],
                b[1],
            ],
        ];
        for row in rows {
            for r in 0..9 {
                for c in 0..9 {
                    ata[r][c] += row[r] * row[c];
                }
            }
        }
    }
    let (values, vectors) = symmetric_eigen(ata);
    let smallest = (0..9).min_by(|&a, &b| values[a].total_cmp(&values[b]))?;
    let hn: Mat3 = std::array::from_fn(|r| std::array::from_fn(|c| vectors[r * 3 + c][smallest]));
    let t2_inverse = invert(&t2)?;
    let h = mul(&mul(&t2_inverse, &hn), &t1);
    let scale = if h[2][2].abs() > 1e-12 { h[2][2] } else { 1.0 };
    let h = h.map(|row| row.map(|v| v / scale));
    h.iter().flatten().all(|v| v.is_finite()).then_some(h)
}
