//! Scale-invariant keypoints and descriptors (Lowe, "Distinctive Image Features from
//! Scale-Invariant Keypoints", IJCV 2004).
//!
//! Ported from OpenCV 4.x `modules/features2d/src/sift.dispatch.cpp` and `sift.simd.hpp`
//! (Apache-2.0), which derive from Rob Hess's SIFT implementation under the notice
//! below; US patent 6,711,293 expired in March 2020. Differences from OpenCV: the scalar
//! path only; the input is a float image in [0, 1] rather than 0–255, so the 1/255 scale
//! factors drop out; the image is not doubled first (`firstOctave = 0`), as cpfind also
//! works on reduced images; and descriptors are RootSIFT-normalized (Arandjelović &
//! Zisserman, "Three things everyone should know to improve object retrieval", 2012), so
//! the L2 distance between them compares like the Hellinger kernel.
//!
//! Keypoints are spread over the image with cpfind's sieve, and keypoints whose
//! descriptor window reaches pixels the caller masks out are dropped.
//!
//! OpenCV: Copyright (C) 2009, Willow Garage Inc.; Copyright (C) 2020, Intel Corporation.
//!
//! Copyright (c) 2006-2010, Rob Hess <hess@eecs.oregonstate.edu>
//! All rights reserved.
//!
//! Redistribution and use in source and binary forms, with or without modification, are
//! permitted provided that the following conditions are met:
//! * Redistributions of source code must retain the above copyright and patent notices,
//!   this list of conditions and the following disclaimer.
//! * Redistributions in binary form must reproduce the above copyright notice, this list
//!   of conditions and the following disclaimer in the documentation and/or other
//!   materials provided with the distribution.
//! * Neither the name of Oregon State University nor the names of its contributors may
//!   be used to endorse or promote products derived from this software without specific
//!   prior written permission.
//!
//! THIS SOFTWARE IS PROVIDED BY THE COPYRIGHT HOLDERS AND CONTRIBUTORS "AS IS" AND ANY
//! EXPRESS OR IMPLIED WARRANTIES, INCLUDING, BUT NOT LIMITED TO, THE IMPLIED WARRANTIES
//! OF MERCHANTABILITY AND FITNESS FOR A PARTICULAR PURPOSE ARE DISCLAIMED. IN NO EVENT
//! SHALL THE COPYRIGHT HOLDER BE LIABLE FOR ANY DIRECT, INDIRECT, INCIDENTAL, SPECIAL,
//! EXEMPLARY, OR CONSEQUENTIAL DAMAGES (INCLUDING, BUT NOT LIMITED TO, PROCUREMENT OF
//! SUBSTITUTE GOODS OR SERVICES; LOSS OF USE, DATA, OR PROFITS; OR BUSINESS
//! INTERRUPTION) HOWEVER CAUSED AND ON ANY THEORY OF LIABILITY, WHETHER IN CONTRACT,
//! STRICT LIABILITY, OR TORT (INCLUDING NEGLIGENCE OR OTHERWISE) ARISING IN ANY WAY OUT
//! OF THE USE OF THIS SOFTWARE, EVEN IF ADVISED OF THE POSSIBILITY OF SUCH DAMAGE.
//!
//! The patent notice, the patent having expired, and the full OpenCV terms are in
//! `THIRD_PARTY_NOTICES.md`.

use rayon::prelude::*;

/// Layers per octave (OpenCV default).
const OCTAVE_LAYERS: usize = 3;
/// OpenCV defaults for the extremum tests.
const CONTRAST_THRESHOLD: f32 = 0.04;
const EDGE_THRESHOLD: f32 = 10.0;
/// Blur of the octave's first layer.
const SIGMA: f32 = 1.6;
/// Assumed blur of the input image.
const INIT_SIGMA: f32 = 0.5;
const IMG_BORDER: usize = 5;
const MAX_INTERP_STEPS: usize = 5;
const ORI_HIST_BINS: usize = 36;
const ORI_SIG_FCTR: f32 = 1.5;
const ORI_RADIUS: f32 = 3.0 * ORI_SIG_FCTR;
const ORI_PEAK_RATIO: f32 = 0.8;
const DESCR_WIDTH: usize = 4;
const DESCR_HIST_BINS: usize = 8;
const DESCR_SCL_FCTR: f32 = 3.0;
const DESCR_MAG_THR: f32 = 0.2;
pub(super) const DESCRIPTOR_LENGTH: usize = DESCR_WIDTH * DESCR_WIDTH * DESCR_HIST_BINS;
/// Keypoints kept per image, spread by cpfind's 10 × 10 sieve.
const MAX_KEYPOINTS: usize = 4000;
const SIEVE_BUCKETS: usize = 10;
/// A keypoint is dropped when more than this share of its descriptor window is masked.
const MAX_MASKED_SHARE: f32 = 0.05;

pub(super) type Descriptor = [f32; DESCRIPTOR_LENGTH];

/// A keypoint in input image pixels (centers at integer coordinates).
#[derive(Clone, Copy, Debug)]
pub(super) struct Keypoint {
    pub(super) x: f32,
    pub(super) y: f32,
    pub(super) response: f32,
}

/// A grayscale image, row-major.
pub(super) struct Gray {
    pub(super) width: usize,
    pub(super) height: usize,
    pub(super) data: Vec<f32>,
}

impl Gray {
    fn at(&self, x: usize, y: usize) -> f32 {
        self.data[y * self.width + x]
    }

    /// Separable Gaussian blur with OpenCV's kernel extent (about 4σ) and its default
    /// reflect-101 border.
    fn blur(&self, sigma: f32) -> Self {
        let radius = (4.0 * sigma).ceil().max(1.0) as isize;
        let mut kernel: Vec<f32> = (-radius..=radius)
            .map(|i| (-(i * i) as f32 / (2.0 * sigma * sigma)).exp())
            .collect();
        let total: f32 = kernel.iter().sum();
        kernel.iter_mut().for_each(|k| *k /= total);
        let reflect = |i: isize, size: usize| -> usize {
            let size = size as isize;
            if size == 1 {
                return 0;
            }
            let mut i = i;
            while i < 0 || i >= size {
                i = if i < 0 { -i } else { 2 * (size - 1) - i };
            }
            i as usize
        };
        let (w, h) = (self.width, self.height);
        let mut rows = vec![0.0f32; w * h];
        rows.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
            let source = &self.data[y * w..(y + 1) * w];
            for (x, out) in row.iter_mut().enumerate() {
                *out = kernel
                    .iter()
                    .enumerate()
                    .map(|(k, weight)| {
                        weight * source[reflect(x as isize + k as isize - radius, w)]
                    })
                    .sum();
            }
        });
        let mut data = vec![0.0f32; w * h];
        data.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
            for (k, weight) in kernel.iter().enumerate() {
                let source = reflect(y as isize + k as isize - radius, h) * w;
                for (out, value) in row.iter_mut().zip(&rows[source..source + w]) {
                    *out += weight * value;
                }
            }
        });
        Self {
            width: w,
            height: h,
            data,
        }
    }

    /// Every second pixel (OpenCV's nearest-neighbour halving between octaves).
    fn halve(&self) -> Self {
        let (w, h) = (self.width / 2, self.height / 2);
        let data = (0..w * h)
            .map(|i| self.at(2 * (i % w), 2 * (i / w)))
            .collect();
        Self {
            width: w,
            height: h,
            data,
        }
    }

    fn difference(&self, other: &Self) -> Self {
        Self {
            width: self.width,
            height: self.height,
            data: other
                .data
                .iter()
                .zip(&self.data)
                .map(|(b, a)| b - a)
                .collect(),
        }
    }
}

/// Keypoints of `image` with their descriptors. `valid` marks the pixels that measured
/// the scene; the image must already be filled where it is false.
pub(super) fn detect(image: &Gray, valid: &[bool]) -> (Vec<Keypoint>, Vec<Descriptor>) {
    let shorter = image.width.min(image.height);
    if shorter < 4 * IMG_BORDER {
        return (Vec::new(), Vec::new());
    }
    let octaves = ((shorter as f32).log2().round() as usize)
        .saturating_sub(2)
        .max(1);
    let sigmas = layer_sigmas();
    let masked = MaskIntegral::new(image.width, image.height, valid);
    let mut base = image.blur((SIGMA * SIGMA - INIT_SIGMA * INIT_SIGMA).max(0.01).sqrt());
    let mut found: Vec<(Keypoint, Descriptor)> = Vec::new();
    for octave in 0..octaves {
        if base.width.min(base.height) < 4 * IMG_BORDER {
            break;
        }
        let mut gauss = Vec::with_capacity(OCTAVE_LAYERS + 3);
        gauss.push(base);
        for sigma in &sigmas[1..] {
            let next = gauss.last().map(|g: &Gray| g.blur(*sigma));
            gauss.extend(next);
        }
        let dog: Vec<Gray> = gauss.windows(2).map(|w| w[0].difference(&w[1])).collect();
        found.extend(octave_keypoints(octave, &gauss, &dog, &masked));
        base = gauss[OCTAVE_LAYERS].halve();
    }
    sieve(image.width, image.height, found).into_iter().unzip()
}

/// OpenCV's incremental blurs: layer `i` is layer `i − 1` blurred by `sig[i]`, so layer
/// `i` has total blur `σ·kⁱ` with `k = 2^(1/layers)`.
fn layer_sigmas() -> [f32; OCTAVE_LAYERS + 3] {
    let k = 2f64.powf(1.0 / OCTAVE_LAYERS as f64);
    std::array::from_fn(|i| {
        if i == 0 {
            SIGMA
        } else {
            let previous = k.powi(i as i32 - 1) * f64::from(SIGMA);
            let total = previous * k;
            (total * total - previous * previous).sqrt() as f32
        }
    })
}

/// Keypoints and descriptors of one octave, in input image pixels.
fn octave_keypoints(
    octave: usize,
    gauss: &[Gray],
    dog: &[Gray],
    masked: &MaskIntegral,
) -> Vec<(Keypoint, Descriptor)> {
    let threshold = 0.5 * CONTRAST_THRESHOLD / OCTAVE_LAYERS as f32;
    let (w, h) = (dog[0].width, dog[0].height);
    let candidates: Vec<(usize, usize, usize)> = (IMG_BORDER..h - IMG_BORDER)
        .into_par_iter()
        .flat_map_iter(|r| {
            let mut row = Vec::new();
            for layer in 1..=OCTAVE_LAYERS {
                for c in IMG_BORDER..w - IMG_BORDER {
                    if is_extremum(&dog[layer - 1..=layer + 1], c, r, threshold) {
                        row.push((layer, r, c));
                    }
                }
            }
            row
        })
        .collect();
    let scale = (1usize << octave) as f32;
    candidates
        .into_par_iter()
        .flat_map_iter(|(layer, r, c)| {
            let mut out = Vec::new();
            let Some(extremum) = refine_extremum(dog, layer, r, c) else {
                return out;
            };
            // Keypoint scale in this octave's pixels.
            let scl =
                SIGMA * 2f32.powf((extremum.layer as f32 + extremum.xi) / OCTAVE_LAYERS as f32);
            let x = (extremum.c as f32 + extremum.xc) * scale;
            let y = (extremum.r as f32 + extremum.xr) * scale;
            let window = DESCR_SCL_FCTR * scl * (DESCR_WIDTH as f32 + 1.0) * 0.5 * scale;
            if masked.share(x, y, window) > MAX_MASKED_SHARE {
                return out;
            }
            let image = &gauss[extremum.layer];
            let hist = orientation_histogram(
                image,
                extremum.c,
                extremum.r,
                (ORI_RADIUS * scl).round() as isize,
                ORI_SIG_FCTR * scl,
            );
            let peak = hist.iter().copied().fold(0.0f32, f32::max);
            let n = ORI_HIST_BINS;
            for j in 0..n {
                let (l, r2) = ((j + n - 1) % n, (j + 1) % n);
                if hist[j] > hist[l] && hist[j] > hist[r2] && hist[j] >= peak * ORI_PEAK_RATIO {
                    let mut bin = j as f32
                        + 0.5 * (hist[l] - hist[r2]) / (hist[l] - 2.0 * hist[j] + hist[r2]);
                    if bin < 0.0 {
                        bin += n as f32;
                    } else if bin >= n as f32 {
                        bin -= n as f32;
                    }
                    let angle = 360.0 / n as f32 * bin;
                    let descriptor = describe(
                        image,
                        extremum.c as f32 + extremum.xc,
                        extremum.r as f32 + extremum.xr,
                        angle,
                        scl,
                    );
                    out.push((
                        Keypoint {
                            x,
                            y,
                            response: extremum.contrast.abs(),
                        },
                        descriptor,
                    ));
                }
            }
            out
        })
        .collect()
}

/// Whether `dog[1]` at `(c, r)` exceeds the threshold and is the maximum (or minimum)
/// of its 26 neighbours across three layers.
fn is_extremum(dog: &[Gray], c: usize, r: usize, threshold: f32) -> bool {
    let value = dog[1].at(c, r);
    if value.abs() <= threshold {
        return false;
    }
    let neighbours = dog.iter().enumerate().flat_map(|(layer, image)| {
        (r - 1..=r + 1).flat_map(move |y| {
            (c - 1..=c + 1)
                .filter(move |&x| layer != 1 || x != c || y != r)
                .map(move |x| image.at(x, y))
        })
    });
    if value > 0.0 {
        neighbours.into_iter().all(|n| value >= n)
    } else {
        neighbours.into_iter().all(|n| value <= n)
    }
}

/// A refined scale-space extremum.
struct Extremum {
    layer: usize,
    r: usize,
    c: usize,
    xi: f32,
    xr: f32,
    xc: f32,
    contrast: f32,
}

/// OpenCV's `adjustLocalExtrema`: interpolates the extremum to subpixel position and
/// scale (Lowe §4) and rejects low contrast and edge responses.
fn refine_extremum(dog: &[Gray], layer: usize, r: usize, c: usize) -> Option<Extremum> {
    let (mut layer, mut r, mut c) = (layer as isize, r as isize, c as isize);
    let (w, h) = (dog[0].width as isize, dog[0].height as isize);
    let border = IMG_BORDER as isize;
    let at = |l: isize, x: isize, y: isize| dog[l as usize].at(x as usize, y as usize);
    let derivatives = |l: isize, r: isize, c: isize| {
        let d = [
            (at(l, c + 1, r) - at(l, c - 1, r)) * 0.5,
            (at(l, c, r + 1) - at(l, c, r - 1)) * 0.5,
            (at(l + 1, c, r) - at(l - 1, c, r)) * 0.5,
        ];
        let v2 = at(l, c, r) * 2.0;
        let dxx = at(l, c + 1, r) + at(l, c - 1, r) - v2;
        let dyy = at(l, c, r + 1) + at(l, c, r - 1) - v2;
        let dss = at(l + 1, c, r) + at(l - 1, c, r) - v2;
        let dxy = (at(l, c + 1, r + 1) - at(l, c - 1, r + 1) - at(l, c + 1, r - 1)
            + at(l, c - 1, r - 1))
            * 0.25;
        let dxs = (at(l + 1, c + 1, r) - at(l + 1, c - 1, r) - at(l - 1, c + 1, r)
            + at(l - 1, c - 1, r))
            * 0.25;
        let dys = (at(l + 1, c, r + 1) - at(l + 1, c, r - 1) - at(l - 1, c, r + 1)
            + at(l - 1, c, r - 1))
            * 0.25;
        (d, [[dxx, dxy, dxs], [dxy, dyy, dys], [dxs, dys, dss]])
    };
    let mut offset = [0.0f32; 3];
    let mut converged = false;
    for _ in 0..MAX_INTERP_STEPS {
        let (d, hessian) = derivatives(layer, r, c);
        let x = solve3(hessian, d)?;
        offset = [-x[0], -x[1], -x[2]];
        if offset.iter().all(|v| v.abs() < 0.5) {
            converged = true;
            break;
        }
        if offset.iter().any(|v| v.abs() > (i32::MAX / 3) as f32) {
            return None;
        }
        c += offset[0].round() as isize;
        r += offset[1].round() as isize;
        layer += offset[2].round() as isize;
        if layer < 1
            || layer > OCTAVE_LAYERS as isize
            || c < border
            || c >= w - border
            || r < border
            || r >= h - border
        {
            return None;
        }
    }
    if !converged {
        return None;
    }
    let (d, hessian) = derivatives(layer, r, c);
    let t = d[0] * offset[0] + d[1] * offset[1] + d[2] * offset[2];
    let contrast = at(layer, c, r) + t * 0.5;
    if contrast.abs() * (OCTAVE_LAYERS as f32) < CONTRAST_THRESHOLD {
        return None;
    }
    // Principal curvatures from the trace and determinant of the spatial Hessian.
    let (dxx, dyy, dxy) = (hessian[0][0], hessian[1][1], hessian[0][1]);
    let trace = dxx + dyy;
    let det = dxx * dyy - dxy * dxy;
    if det <= 0.0 || trace * trace * EDGE_THRESHOLD >= (EDGE_THRESHOLD + 1.0).powi(2) * det {
        return None;
    }
    Some(Extremum {
        layer: layer as usize,
        r: r as usize,
        c: c as usize,
        xi: offset[2],
        xr: offset[1],
        xc: offset[0],
        contrast,
    })
}

/// Solves the 3 × 3 system `a·x = b` by Cramer's rule; `None` when singular.
fn solve3(a: [[f32; 3]; 3], b: [f32; 3]) -> Option<[f32; 3]> {
    let det3 = |m: [[f32; 3]; 3]| {
        m[0][0] * (m[1][1] * m[2][2] - m[1][2] * m[2][1])
            - m[0][1] * (m[1][0] * m[2][2] - m[1][2] * m[2][0])
            + m[0][2] * (m[1][0] * m[2][1] - m[1][1] * m[2][0])
    };
    let det = det3(a);
    if det.abs() < 1e-20 || !det.is_finite() {
        return None;
    }
    Some(std::array::from_fn(|k| {
        let mut m = a;
        for row in 0..3 {
            m[row][k] = b[row];
        }
        det3(m) / det
    }))
}

/// OpenCV's `calcOrientationHist`: a Gaussian-weighted histogram of gradient directions
/// around `(x, y)`, smoothed with `[1 4 6 4 1] / 16`.
fn orientation_histogram(
    image: &Gray,
    x: usize,
    y: usize,
    radius: isize,
    sigma: f32,
) -> [f32; ORI_HIST_BINS] {
    let n = ORI_HIST_BINS;
    let exp_scale = -1.0 / (2.0 * sigma * sigma);
    let mut raw = [0.0f32; ORI_HIST_BINS];
    for i in -radius..=radius {
        let yy = y as isize + i;
        if yy <= 0 || yy >= image.height as isize - 1 {
            continue;
        }
        for j in -radius..=radius {
            let xx = x as isize + j;
            if xx <= 0 || xx >= image.width as isize - 1 {
                continue;
            }
            let (xx, yy) = (xx as usize, yy as usize);
            let dx = image.at(xx + 1, yy) - image.at(xx - 1, yy);
            let dy = image.at(xx, yy - 1) - image.at(xx, yy + 1);
            let weight = (((i * i + j * j) as f32) * exp_scale).exp();
            let degrees = dy.atan2(dx).to_degrees().rem_euclid(360.0);
            let mut bin = (n as f32 / 360.0 * degrees).round() as isize;
            if bin >= n as isize {
                bin -= n as isize;
            }
            if bin < 0 {
                bin += n as isize;
            }
            raw[bin as usize] += weight * dx.hypot(dy);
        }
    }
    std::array::from_fn(|i| {
        let at = |k: isize| raw[(i as isize + k).rem_euclid(n as isize) as usize];
        (at(-2) + at(2)) * (1.0 / 16.0) + (at(-1) + at(1)) * (4.0 / 16.0) + at(0) * (6.0 / 16.0)
    })
}

/// OpenCV's `calcSIFTDescriptor` at octave position `(x, y)`, orientation `angle`
/// (degrees) and scale `scl`, followed by RootSIFT normalization.
fn describe(image: &Gray, x: f32, y: f32, angle: f32, scl: f32) -> Descriptor {
    let d = DESCR_WIDTH;
    let n = DESCR_HIST_BINS;
    let (px, py) = (x.round() as isize, y.round() as isize);
    let (sin, cos) = angle.to_radians().sin_cos();
    let bins_per_degree = n as f32 / 360.0;
    let exp_scale = -1.0 / (d as f32 * d as f32 * 0.5);
    let hist_width = DESCR_SCL_FCTR * scl;
    let diagonal = (image.width as f32).hypot(image.height as f32);
    let radius = ((hist_width * std::f32::consts::SQRT_2 * (d as f32 + 1.0) * 0.5).round())
        .min(diagonal) as isize;
    let (cos, sin) = (cos / hist_width, sin / hist_width);
    let mut hist = vec![0.0f32; (d + 2) * (d + 2) * (n + 2)];
    for i in -radius..=radius {
        for j in -radius..=radius {
            // Histogram coordinates rotated relative to the orientation; the 0.5 puts a
            // sample at a bin's center fully into that bin after interpolation.
            let c_rot = j as f32 * cos - i as f32 * sin;
            let r_rot = j as f32 * sin + i as f32 * cos;
            let rbin = r_rot + d as f32 / 2.0 - 0.5;
            let cbin = c_rot + d as f32 / 2.0 - 0.5;
            let (r, c) = (py + i, px + j);
            if !(rbin > -1.0
                && rbin < d as f32
                && cbin > -1.0
                && cbin < d as f32
                && r > 0
                && r < image.height as isize - 1
                && c > 0
                && c < image.width as isize - 1)
            {
                continue;
            }
            let (r, c) = (r as usize, c as usize);
            let dx = image.at(c + 1, r) - image.at(c - 1, r);
            let dy = image.at(c, r - 1) - image.at(c, r + 1);
            let degrees = dy.atan2(dx).to_degrees().rem_euclid(360.0);
            let weight = ((c_rot * c_rot + r_rot * r_rot) * exp_scale).exp();
            let magnitude = dx.hypot(dy) * weight;
            let obin = (degrees - angle) * bins_per_degree;
            let (r0, c0, o0) = (rbin.floor(), cbin.floor(), obin.floor());
            let (rbin, cbin, obin) = (rbin - r0, cbin - c0, obin - o0);
            let o0 = (o0 as isize).rem_euclid(n as isize) as usize;
            let (r0, c0) = (r0 as isize, c0 as isize);
            // Trilinear interpolation into the padded histogram.
            let v_r1 = magnitude * rbin;
            let v_r0 = magnitude - v_r1;
            let v_rc11 = v_r1 * cbin;
            let v_rc10 = v_r1 - v_rc11;
            let v_rc01 = v_r0 * cbin;
            let v_rc00 = v_r0 - v_rc01;
            let v_rco111 = v_rc11 * obin;
            let v_rco110 = v_rc11 - v_rco111;
            let v_rco101 = v_rc10 * obin;
            let v_rco100 = v_rc10 - v_rco101;
            let v_rco011 = v_rc01 * obin;
            let v_rco010 = v_rc01 - v_rco011;
            let v_rco001 = v_rc00 * obin;
            let v_rco000 = v_rc00 - v_rco001;
            let idx = (((r0 + 1) as usize * (d + 2) + (c0 + 1) as usize) * (n + 2)) + o0;
            hist[idx] += v_rco000;
            hist[idx + 1] += v_rco001;
            hist[idx + (n + 2)] += v_rco010;
            hist[idx + (n + 3)] += v_rco011;
            hist[idx + (d + 2) * (n + 2)] += v_rco100;
            hist[idx + (d + 2) * (n + 2) + 1] += v_rco101;
            hist[idx + (d + 3) * (n + 2)] += v_rco110;
            hist[idx + (d + 3) * (n + 2) + 1] += v_rco111;
        }
    }
    // The orientation bins are circular.
    let mut descriptor = [0.0f32; DESCRIPTOR_LENGTH];
    for i in 0..d {
        for j in 0..d {
            let idx = ((i + 1) * (d + 2) + (j + 1)) * (n + 2);
            hist[idx] += hist[idx + n];
            hist[idx + 1] += hist[idx + n + 1];
            descriptor[(i * d + j) * n..(i * d + j + 1) * n].copy_from_slice(&hist[idx..idx + n]);
        }
    }
    // Clamp large gradient magnitudes (illumination changes), then RootSIFT.
    let threshold = descriptor.iter().map(|v| v * v).sum::<f32>().sqrt() * DESCR_MAG_THR;
    descriptor.iter_mut().for_each(|v| *v = v.min(threshold));
    let l1 = descriptor.iter().sum::<f32>().max(f32::EPSILON);
    descriptor.iter_mut().for_each(|v| *v = (*v / l1).sqrt());
    descriptor
}

/// Counts masked pixels in squares of the input image.
struct MaskIntegral {
    width: usize,
    height: usize,
    /// `(width + 1) × (height + 1)` summed-area table of masked pixels.
    sums: Vec<u32>,
}

impl MaskIntegral {
    fn new(width: usize, height: usize, valid: &[bool]) -> Self {
        let stride = width + 1;
        let mut sums = vec![0u32; stride * (height + 1)];
        for y in 0..height {
            let mut row = 0u32;
            for x in 0..width {
                row += u32::from(!valid[y * width + x]);
                sums[(y + 1) * stride + x + 1] = sums[y * stride + x + 1] + row;
            }
        }
        Self {
            width,
            height,
            sums,
        }
    }

    /// Share of masked pixels within `radius` of `(x, y)`; pixels outside the image
    /// count as masked.
    fn share(&self, x: f32, y: f32, radius: f32) -> f32 {
        let lo = |v: f32| (v - radius).floor().max(0.0) as usize;
        let hi = |v: f32, size: usize| ((v + radius).ceil() as usize + 1).min(size);
        let (x0, y0, x1, y1) = (lo(x), lo(y), hi(x, self.width), hi(y, self.height));
        let side = (2.0 * radius.ceil() + 1.0).powi(2);
        if x1 <= x0 || y1 <= y0 {
            return 1.0;
        }
        let stride = self.width + 1;
        let at = |x: usize, y: usize| self.sums[y * stride + x];
        let masked = at(x1, y1) + at(x0, y0) - at(x0, y1) - at(x1, y0);
        let inside = ((x1 - x0) * (y1 - y0)) as f32;
        (masked as f32 + (side - inside).max(0.0)) / side
    }
}

/// cpfind's first sieve: the strongest keypoints of each of `SIEVE_BUCKETS²` cells, so
/// matches spread over the image instead of crowding into its most textured part.
fn sieve(
    width: usize,
    height: usize,
    mut found: Vec<(Keypoint, Descriptor)>,
) -> Vec<(Keypoint, Descriptor)> {
    let per_bucket = MAX_KEYPOINTS / (SIEVE_BUCKETS * SIEVE_BUCKETS);
    found.sort_by(|a, b| b.0.response.total_cmp(&a.0.response));
    let mut counts = vec![0usize; SIEVE_BUCKETS * SIEVE_BUCKETS];
    found
        .into_iter()
        .filter(|(point, _)| {
            let bx = ((point.x.max(0.0) as usize * SIEVE_BUCKETS) / width).min(SIEVE_BUCKETS - 1);
            let by = ((point.y.max(0.0) as usize * SIEVE_BUCKETS) / height).min(SIEVE_BUCKETS - 1);
            let count = &mut counts[by * SIEVE_BUCKETS + bx];
            *count += 1;
            *count <= per_bucket
        })
        .collect()
}
