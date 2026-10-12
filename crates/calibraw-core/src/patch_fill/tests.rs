use super::*;

struct Fixture {
    width: usize,
    height: usize,
    pixels: Vec<[f32; 3]>,
    hole: Vec<bool>,
    source: Vec<bool>,
}

impl Fixture {
    fn new(width: usize, height: usize, value: impl Fn(usize, usize) -> [f32; 3]) -> Self {
        Self {
            width,
            height,
            pixels: (0..width * height)
                .map(|index| value(index % width, index / width))
                .collect(),
            hole: vec![false; width * height],
            source: vec![true; width * height],
        }
    }

    fn with_hole(mut self, left: usize, top: usize, width: usize, height: usize) -> Self {
        for y in top..top + height {
            for x in left..left + width {
                self.hole[y * self.width + x] = true;
            }
        }
        self
    }

    fn fill_in(&self, space: &impl PerceptualSpace, settings: PatchFillSettings) -> PatchFill {
        let image = PatchFillImage::new(
            self.width,
            self.height,
            &self.pixels,
            &self.hole,
            &self.source,
        )
        .unwrap();
        patch_fill(
            &image,
            space,
            settings,
            &AtomicBool::new(false),
            &mut |_| {},
        )
        .unwrap()
    }

    fn fill(&self) -> PatchFill {
        self.fill_in(&IdentitySpace, PatchFillSettings::default())
    }

    fn guided_fill(&self, guide: &[[f32; 3]], detail: f32) -> PatchFill {
        let image = PatchFillImage::new(
            self.width,
            self.height,
            &self.pixels,
            &self.hole,
            &self.source,
        )
        .unwrap()
        .with_guide(guide, detail)
        .unwrap();
        patch_fill(
            &image,
            &IdentitySpace,
            PatchFillSettings::default(),
            &AtomicBool::new(false),
            &mut |_| {},
        )
        .unwrap()
    }

    fn hole_values<'a>(&self, fill: &'a PatchFill) -> impl Iterator<Item = &'a FilledPixel> {
        fill.pixels().iter().filter(|pixel| pixel.distance == 0)
    }
}

/// Deterministic pseudo-random value in [-amplitude, amplitude].
fn noise(index: usize, amplitude: f32) -> f32 {
    (field_noise(index as u64) as f64 / u64::MAX as f64 * 2.0 - 1.0) as f32 * amplitude
}

fn field_noise(mut value: u64) -> u64 {
    value = value.wrapping_add(0x9E37_79B9_7F4A_7C15);
    value = (value ^ (value >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    value = (value ^ (value >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    value ^ (value >> 31)
}

fn deviation(values: &[f32]) -> f32 {
    let mean = values.iter().sum::<f32>() / values.len() as f32;
    (values
        .iter()
        .map(|value| (value - mean).powi(2))
        .sum::<f32>()
        / values.len() as f32)
        .sqrt()
}

#[test]
fn flat_image_is_filled_with_its_colour() {
    let fixture = Fixture::new(64, 48, |_, _| [0.3, 0.5, 0.7]).with_hole(20, 14, 18, 16);
    let fill = fixture.fill();
    assert!(!fill.is_empty());
    for pixel in fill.pixels() {
        for (channel, expected) in [0.3, 0.5, 0.7].into_iter().enumerate() {
            assert!((pixel.value[channel] - expected).abs() < 1e-5, "{pixel:?}");
        }
    }
}

#[test]
fn fill_covers_exactly_the_hole_and_its_band() {
    let fixture =
        Fixture::new(80, 60, |x, y| [noise(y * 80 + x, 0.1) + 0.5; 3]).with_hole(30, 20, 12, 9);
    let fill = fixture.fill();
    let mut expected = Vec::new();
    for y in 0..60usize {
        for x in 0..80usize {
            let distance = (30usize.saturating_sub(x).max(x.saturating_sub(41)))
                .max(20usize.saturating_sub(y).max(y.saturating_sub(28)));
            if distance <= PATCH_FILL_BAND {
                expected.push((x, y, distance));
            }
        }
    }
    let actual = fill
        .pixels()
        .iter()
        .map(|pixel| (pixel.x, pixel.y, pixel.distance))
        .collect::<Vec<_>>();
    assert_eq!(actual, expected);
}

#[test]
fn periodic_texture_continues_through_the_hole() {
    let stripe = |x: usize| if (x / 4).is_multiple_of(2) { 0.2 } else { 0.8 };
    let fixture = Fixture::new(128, 96, |x, _| [stripe(x); 3]).with_hole(50, 36, 22, 22);
    let fill = fixture.fill();
    let errors = fixture
        .hole_values(&fill)
        .map(|pixel| (pixel.value[0] - stripe(pixel.x)).abs())
        .collect::<Vec<_>>();
    let mean = errors.iter().sum::<f32>() / errors.len() as f32;
    assert!(mean < 0.02, "stripes not continued: mean error {mean}");
}

#[test]
fn grain_is_copied_rather_than_averaged_away() {
    let fixture =
        Fixture::new(128, 128, |x, y| [0.5 + noise(y * 128 + x, 0.1); 3]).with_hole(48, 48, 32, 32);
    let fill = fixture.fill();
    let filled = fixture
        .hole_values(&fill)
        .map(|pixel| pixel.value[1])
        .collect::<Vec<_>>();
    let around = (0..128 * 128)
        .filter(|index| !fixture.hole[*index])
        .map(|index| fixture.pixels[index][1])
        .collect::<Vec<_>>();
    let ratio = deviation(&filled) / deviation(&around);
    assert!(
        (0.85..1.15).contains(&ratio),
        "fill grain is {ratio} of the surroundings"
    );
}

#[test]
fn colour_adaptation_continues_smooth_gradients() {
    let ramp = |x: usize, y: usize| 0.15 + 0.6 * x as f32 / 127.0 + 0.1 * y as f32 / 95.0;
    let fixture =
        Fixture::new(128, 96, |x, y| [ramp(x, y), ramp(x, y) * 0.8, 0.4]).with_hole(40, 30, 36, 30);
    let error = |fill: &PatchFill| {
        let errors = fixture
            .hole_values(fill)
            .map(|pixel| (pixel.value[0] - ramp(pixel.x, pixel.y)).abs())
            .collect::<Vec<_>>();
        errors.iter().sum::<f32>() / errors.len() as f32
    };
    let adapted = error(&fixture.fill());
    let unadapted = error(&fixture.fill_in(
        &IdentitySpace,
        PatchFillSettings {
            color_adaptation: false,
            ..PatchFillSettings::default()
        },
    ));
    assert!(adapted < 0.01, "adapted gradient error {adapted}");
    assert!(adapted <= unadapted, "{adapted} > {unadapted}");
}

#[test]
fn guided_fill_follows_the_guide_structure() {
    // A dark block far from the hole is the only dark material; the guide
    // asks for it in the middle of the hole, where an unguided fill would
    // continue the light surroundings.
    let dark = |x: usize, y: usize| (100..124).contains(&x) && (4..28).contains(&y);
    let fixture = Fixture::new(128, 128, |x, y| {
        let base = if dark(x, y) { 0.15 } else { 0.6 };
        [base + noise(y * 128 + x, 0.05); 3]
    })
    .with_hole(44, 44, 40, 40);
    let inner = |x: usize, y: usize| (56..72).contains(&x) && (56..72).contains(&y);
    let guide = (0..128 * 128)
        .map(|index| {
            let (x, y) = (index % 128, index / 128);
            if fixture.hole[index] {
                [if inner(x, y) { 0.15 } else { 0.6 }; 3]
            } else {
                fixture.pixels[index]
            }
        })
        .collect::<Vec<_>>();
    let fill = fixture.guided_fill(&guide, 2.0);
    let mean = |region: &dyn Fn(usize, usize) -> bool| {
        let values = fixture
            .hole_values(&fill)
            .filter(|pixel| region(pixel.x, pixel.y))
            .map(|pixel| pixel.value[0])
            .collect::<Vec<_>>();
        values.iter().sum::<f32>() / values.len() as f32
    };
    let centre = mean(&|x, y| (60..68).contains(&x) && (60..68).contains(&y));
    let rim = mean(&|x, y| !(52..76).contains(&x) || !(52..76).contains(&y));
    assert!(centre < 0.3, "guided centre is {centre}");
    assert!(rim > 0.5, "guided rim is {rim}");
}

#[test]
fn guided_fill_copies_image_texture_instead_of_the_guide() {
    let fixture =
        Fixture::new(128, 128, |x, y| [0.5 + noise(y * 128 + x, 0.1); 3]).with_hole(48, 48, 32, 32);
    let guide = vec![[0.5f32; 3]; 128 * 128];
    let fill = fixture.guided_fill(&guide, 4.0);
    let filled = fixture
        .hole_values(&fill)
        .map(|pixel| pixel.value[1])
        .collect::<Vec<_>>();
    let around = (0..128 * 128)
        .filter(|index| !fixture.hole[*index])
        .map(|index| fixture.pixels[index][1])
        .collect::<Vec<_>>();
    let ratio = deviation(&filled) / deviation(&around);
    assert!(
        (0.85..1.15).contains(&ratio),
        "guided fill grain is {ratio} of the surroundings"
    );
}

#[test]
fn guided_start_follows_the_guide_detail_within_the_pyramid() {
    assert_eq!(guided_start_level(1.0, 4), 1);
    assert_eq!(guided_start_level(2.0, 4), 1);
    assert_eq!(guided_start_level(2.5, 4), 2);
    assert_eq!(guided_start_level(8.0, 4), 3);
    assert_eq!(guided_start_level(64.0, 4), 3);
    assert_eq!(guided_start_level(4.0, 1), 0);
}

#[test]
fn invalid_guides_are_rejected() {
    let pixels = vec![[0.5f32; 3]; 12];
    let mut hole = vec![false; 12];
    hole[5] = true;
    let source = vec![true; 12];
    let image = PatchFillImage::new(4, 3, &pixels, &hole, &source).unwrap();
    assert!(image.with_guide(&pixels[..11], 2.0).is_err());
    assert!(image.with_guide(&pixels, 0.0).is_err());
    let mut bad = pixels.clone();
    bad[5][0] = f32::INFINITY;
    assert!(image.with_guide(&bad, 2.0).is_err());
    bad[5][0] = 0.5;
    bad[0][0] = f32::NAN;
    assert!(image.with_guide(&bad, 2.0).is_ok());
}

/// Matches on the first channel only and carries each pixel's position in the
/// others, so a fill reveals where every value was copied from.
struct PositionSpace;

impl PerceptualSpace for PositionSpace {
    fn encode(&self, value: [f32; 3]) -> [f32; 3] {
        [value[0]; 3]
    }

    fn decode(&self, perceptual: [f32; 3]) -> [f32; 3] {
        perceptual
    }
}

#[test]
fn values_are_only_copied_from_allowed_sources() {
    let checker = |x: usize, y: usize| {
        if (x / 3 + y / 3).is_multiple_of(2) {
            0.25
        } else {
            0.75
        }
    };
    let mut fixture =
        Fixture::new(96, 64, |x, y| [checker(x, y), x as f32, y as f32]).with_hole(30, 20, 16, 16);
    for y in 0..64 {
        for x in 0..48 {
            fixture.source[y * 96 + x] = false;
        }
    }
    let fill = fixture.fill_in(
        &PositionSpace,
        PatchFillSettings {
            color_adaptation: false,
            ..PatchFillSettings::default()
        },
    );
    for pixel in fill.pixels() {
        let (x, y) = (pixel.value[1] as usize, pixel.value[2] as usize);
        let index = y * 96 + x;
        assert!(
            fixture.source[index] && !fixture.hole[index],
            "{pixel:?} copied from a forbidden pixel"
        );
    }
}

#[test]
fn equal_inputs_and_seeds_give_equal_fills() {
    let fixture =
        Fixture::new(96, 80, |x, y| [0.4 + noise(y * 96 + x, 0.2); 3]).with_hole(30, 25, 30, 20);
    assert_eq!(fixture.fill(), fixture.fill());
}

#[test]
fn cancellation_and_missing_sources_are_reported() {
    let fixture = Fixture::new(64, 64, |_, _| [0.5; 3]).with_hole(20, 20, 10, 10);
    let image =
        PatchFillImage::new(64, 64, &fixture.pixels, &fixture.hole, &fixture.source).unwrap();
    let cancelled = patch_fill(
        &image,
        &IdentitySpace,
        PatchFillSettings::default(),
        &AtomicBool::new(true),
        &mut |_| {},
    );
    assert_eq!(cancelled, Err(PatchFillError::Cancelled));

    let nowhere = vec![false; 64 * 64];
    let image = PatchFillImage::new(64, 64, &fixture.pixels, &fixture.hole, &nowhere).unwrap();
    let starved = patch_fill(
        &image,
        &IdentitySpace,
        PatchFillSettings::default(),
        &AtomicBool::new(false),
        &mut |_| {},
    );
    assert_eq!(starved, Err(PatchFillError::NoSource));
}

#[test]
fn invalid_images_are_rejected() {
    let pixels = vec![[0.5f32; 3]; 12];
    let masks = vec![false; 12];
    assert!(PatchFillImage::new(4, 4, &pixels, &masks, &masks).is_err());
    let mut bad = pixels.clone();
    bad[3][1] = f32::NAN;
    assert!(PatchFillImage::new(4, 3, &bad, &masks, &masks).is_err());
    assert!(PatchFillImage::new(4, 3, &pixels, &masks, &masks).is_ok());
}

#[test]
fn progress_rises_to_completion() {
    let fixture =
        Fixture::new(160, 120, |x, y| [0.5 + noise(y * 160 + x, 0.1); 3]).with_hole(50, 40, 60, 40);
    let image =
        PatchFillImage::new(160, 120, &fixture.pixels, &fixture.hole, &fixture.source).unwrap();
    let mut reported = Vec::new();
    patch_fill(
        &image,
        &IdentitySpace,
        PatchFillSettings::default(),
        &AtomicBool::new(false),
        &mut |fraction| reported.push(fraction),
    )
    .unwrap();
    assert!(reported.windows(2).all(|pair| pair[0] <= pair[1]));
    assert_eq!(reported.last(), Some(&1.0));
}

#[test]
fn membrane_interpolates_linearly_between_known_edges() {
    let (width, height) = (200, 40);
    let mut values = vec![[0.0f32; 3]; width * height];
    let mut unknown = vec![true; width * height];
    for y in 0..height {
        values[y * width + width - 1] = [1.0, 2.0, -1.0];
        unknown[y * width] = false;
        unknown[y * width + width - 1] = false;
    }
    membrane::fill(&mut values, width, height, &unknown);
    let mut worst = 0.0f32;
    for y in 0..height {
        for x in 0..width {
            let expected = x as f32 / (width - 1) as f32;
            let value = values[y * width + x];
            worst = worst.max((value[0] - expected).abs());
            assert!((value[1] - 2.0 * value[0]).abs() < 1e-4);
            assert!((value[2] + value[0]).abs() < 1e-4);
        }
    }
    // A 0.5% residual of the boundary difference is far below visibility.
    assert!(worst < 5e-3, "membrane error {worst}");
}

#[test]
fn chebyshev_distance_matches_brute_force() {
    let (width, height) = (23, 17);
    let mask = (0..width * height)
        .map(|index| field_noise(index as u64).is_multiple_of(37))
        .collect::<Vec<_>>();
    let distance = distance::chebyshev(&mask, width, height, 6);
    for y in 0..height {
        for x in 0..width {
            let expected = (0..width * height)
                .filter(|index| mask[*index])
                .map(|index| {
                    let (mx, my) = (index % width, index / width);
                    (mx.abs_diff(x)).max(my.abs_diff(y)) as u32
                })
                .min()
                .unwrap_or(u32::MAX)
                .min(6);
            assert_eq!(distance[y * width + x], expected, "{x},{y}");
        }
    }
}
