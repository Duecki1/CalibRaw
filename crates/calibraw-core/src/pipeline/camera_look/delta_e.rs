//! CIELAB (D65 white) and the CIEDE2000 colour difference, which the camera
//! match fit scores and weights its samples with.

/// Linear sRGB (D65) to CIELAB relative to the D65 white.
pub fn linear_srgb_to_lab(rgb: [f32; 3]) -> [f32; 3] {
    let [r, g, b] = rgb;
    let x = 0.412_456_4 * r + 0.357_576_1 * g + 0.180_437_5 * b;
    let y = 0.212_672_9 * r + 0.715_152_2 * g + 0.072_175 * b;
    let z = 0.019_333_9 * r + 0.119_192 * g + 0.950_304_1 * b;
    let fx = lab_f(x / 0.950_47);
    let fy = lab_f(y);
    let fz = lab_f(z / 1.088_83);
    [116.0 * fy - 16.0, 500.0 * (fx - fy), 200.0 * (fy - fz)]
}

fn lab_f(t: f32) -> f32 {
    const DELTA: f32 = 6.0 / 29.0;
    if t > DELTA * DELTA * DELTA {
        t.cbrt()
    } else {
        t / (3.0 * DELTA * DELTA) + 4.0 / 29.0
    }
}

/// CIEDE2000 difference of two CIELAB colours with unit weights
/// (kL = kC = kH = 1), after Sharma, Wu and Dalal (2005).
pub fn ciede2000(first: [f32; 3], second: [f32; 3]) -> f32 {
    let [l1, a1, b1] = first.map(f64::from);
    let [l2, a2, b2] = second.map(f64::from);
    let pow25_7 = 25.0_f64.powi(7);
    let c_mean = (a1.hypot(b1) + a2.hypot(b2)) * 0.5;
    let c_mean7 = c_mean.powi(7);
    let g = 0.5 * (1.0 - (c_mean7 / (c_mean7 + pow25_7)).sqrt());
    let a1p = (1.0 + g) * a1;
    let a2p = (1.0 + g) * a2;
    let c1p = a1p.hypot(b1);
    let c2p = a2p.hypot(b2);
    let hue = |b: f64, a: f64| {
        if b == 0.0 && a == 0.0 {
            0.0
        } else {
            b.atan2(a).to_degrees().rem_euclid(360.0)
        }
    };
    let h1p = hue(b1, a1p);
    let h2p = hue(b2, a2p);

    let delta_l = l2 - l1;
    let delta_c = c2p - c1p;
    let chroma_product = c1p * c2p;
    let delta_h_angle = if chroma_product == 0.0 {
        0.0
    } else {
        let difference = h2p - h1p;
        if difference > 180.0 {
            difference - 360.0
        } else if difference < -180.0 {
            difference + 360.0
        } else {
            difference
        }
    };
    let delta_h = 2.0 * chroma_product.sqrt() * (delta_h_angle.to_radians() * 0.5).sin();

    let l_mean = (l1 + l2) * 0.5;
    let c_mean_p = (c1p + c2p) * 0.5;
    let h_mean = if chroma_product == 0.0 {
        h1p + h2p
    } else if (h1p - h2p).abs() <= 180.0 {
        (h1p + h2p) * 0.5
    } else if h1p + h2p < 360.0 {
        (h1p + h2p + 360.0) * 0.5
    } else {
        (h1p + h2p - 360.0) * 0.5
    };
    let t = 1.0 - 0.17 * (h_mean - 30.0).to_radians().cos()
        + 0.24 * (2.0 * h_mean).to_radians().cos()
        + 0.32 * (3.0 * h_mean + 6.0).to_radians().cos()
        - 0.20 * (4.0 * h_mean - 63.0).to_radians().cos();
    let delta_theta = 30.0 * (-((h_mean - 275.0) / 25.0).powi(2)).exp();
    let c_mean_p7 = c_mean_p.powi(7);
    let r_c = 2.0 * (c_mean_p7 / (c_mean_p7 + pow25_7)).sqrt();
    let l_offset = (l_mean - 50.0).powi(2);
    let s_l = 1.0 + 0.015 * l_offset / (20.0 + l_offset).sqrt();
    let s_c = 1.0 + 0.045 * c_mean_p;
    let s_h = 1.0 + 0.015 * c_mean_p * t;
    let r_t = -(2.0 * delta_theta).to_radians().sin() * r_c;
    let l_term = delta_l / s_l;
    let c_term = delta_c / s_c;
    let h_term = delta_h / s_h;
    (l_term * l_term + c_term * c_term + h_term * h_term + r_t * c_term * h_term)
        .max(0.0)
        .sqrt() as f32
}
