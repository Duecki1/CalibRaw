use super::{direct_shader::run_direct_shader, tests::request_test_device};
use crate::wgpu;

// Runs the production Saturation/Vibrance function on OKLab inputs and returns
// OKLab outputs, so the test measures chroma factors without copying the
// colour conversions.
const DIRECT_SHADER: &str = r#"
#import calibraw::basic_adjustments as BasicAdjustments
#import calibraw::color as Color
struct Params { saturation: f32, vibrance: f32, count: u32, _pad0: u32 }
@group(0) @binding(40) var<uniform> params: Params;
@group(0) @binding(41) var<storage, read> input: array<vec4<f32>>;
@group(0) @binding(42) var<storage, read_write> output: array<vec4<f32>>;
@compute @workgroup_size(64)
fn main(@builtin(global_invocation_id) gid: vec3<u32>) {
    if gid.x >= params.count { return; }
    let rgb = Color::rec2020_from_oklab(input[gid.x].xyz);
    let adjusted = BasicAdjustments::apply_saturation_vibrance_values(
        rgb, params.saturation, params.vibrance);
    output[gid.x] = vec4<f32>(Color::rec2020_to_oklab(adjusted), 1.0);
}
"#;

// Blue, magenta and cyan OKLab hue fractions, away from the protected skin hue
// (0.12) and inside Rec.2020 at L = 0.6 even after Saturation +100.
const HUES: [f32; 3] = [0.5, 0.7, 0.85];
const LIGHTNESS: f32 = 0.6;
const SATURATION_PLUS_100: f32 = 1.647_182;

fn oklab(lightness: f32, chroma: f32, hue: f32) -> [f32; 4] {
    let angle = hue * std::f32::consts::TAU;
    [lightness, chroma * angle.cos(), chroma * angle.sin(), 1.0]
}

fn chroma(lab: [f32; 4]) -> f32 {
    lab[1].hypot(lab[2])
}

/// Returns output chroma divided by input chroma for each input.
fn chroma_factors(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    input: &[[f32; 4]],
    saturation: f32,
    vibrance: f32,
) -> anyhow::Result<Vec<f32>> {
    let output = run_direct_shader(
        device,
        queue,
        "saturation/vibrance direct regression shader",
        "saturation_vibrance_tests.wgsl",
        DIRECT_SHADER,
        [
            saturation.to_bits(),
            vibrance.to_bits(),
            input.len() as u32,
            0,
        ],
        input,
    )?;
    Ok(input
        .iter()
        .zip(&output)
        .map(|(before, after)| chroma(*after) / chroma(*before))
        .collect())
}

#[test]
fn vibrance_boosts_muted_colours_and_leaves_saturated_colours() -> anyhow::Result<()> {
    let Some((device, queue)) = request_test_device() else {
        eprintln!("saturation/vibrance regression skipped: no headless wgpu adapter");
        return Ok(());
    };
    let muted = HUES.map(|hue| oklab(LIGHTNESS, 0.03, hue));
    let saturated = HUES.map(|hue| oklab(LIGHTNESS, 0.15, hue));

    let saturation = chroma_factors(&device, &queue, &muted, 100.0, 0.0)?;
    for factor in saturation {
        assert!(
            (factor - SATURATION_PLUS_100).abs() < 2e-3,
            "Saturation +100 chroma factor {factor}"
        );
    }
    let vibrance_muted = chroma_factors(&device, &queue, &muted, 0.0, 100.0)?;
    for (hue, factor) in HUES.iter().zip(&vibrance_muted) {
        assert!(
            (1.5..=SATURATION_PLUS_100 + 1e-3).contains(factor),
            "Vibrance +100, chroma 0.03, hue {hue}: chroma factor {factor}"
        );
    }
    let vibrance_saturated = chroma_factors(&device, &queue, &saturated, 0.0, 100.0)?;
    for (hue, factor) in HUES.iter().zip(&vibrance_saturated) {
        assert!(
            (0.999..=1.05).contains(factor),
            "Vibrance +100, chroma 0.15, hue {hue}: chroma factor {factor}"
        );
    }

    // Saturation and Vibrance multiply without reaching the former 4x clamp.
    let combined = chroma_factors(&device, &queue, &muted, 100.0, 100.0)?;
    for ((hue, factor), vibrance) in HUES.iter().zip(&combined).zip(&vibrance_muted) {
        let expected = SATURATION_PLUS_100 * vibrance;
        assert!(
            (factor / expected - 1.0).abs() < 2e-3,
            "Saturation and Vibrance +100, hue {hue}: {factor}, expected {expected}"
        );
    }
    Ok(())
}

#[test]
fn vibrance_never_boosts_more_than_saturation() -> anyhow::Result<()> {
    let Some((device, queue)) = request_test_device() else {
        eprintln!("saturation/vibrance regression skipped: no headless wgpu adapter");
        return Ok(());
    };
    let chromas = [0.005, 0.01, 0.02, 0.03, 0.045, 0.06, 0.08, 0.10, 0.12, 0.15];
    let input = HUES
        .iter()
        .flat_map(|&hue| chromas.map(|chroma| oklab(LIGHTNESS, chroma, hue)))
        .collect::<Vec<_>>();
    for value in [10.0, 25.0, 50.0, 75.0, 100.0] {
        let saturation = chroma_factors(&device, &queue, &input, value, 0.0)?;
        let vibrance = chroma_factors(&device, &queue, &input, 0.0, value)?;
        for (lab, (vibrance, saturation)) in input.iter().zip(vibrance.iter().zip(&saturation)) {
            let chroma = chroma(*lab);
            assert!(
                *vibrance >= 1.0 - 1e-3 && *vibrance <= saturation + 1e-4,
                "value {value}, chroma {chroma}: Vibrance {vibrance}, Saturation {saturation}"
            );
        }
        // Output chroma keeps the input order within each hue.
        for (hue, factors) in HUES.iter().zip(vibrance.chunks(chromas.len())) {
            let output = chromas
                .iter()
                .zip(factors)
                .map(|(chroma, factor)| chroma * factor)
                .collect::<Vec<_>>();
            assert!(
                output.windows(2).all(|pair| pair[1] > pair[0]),
                "value {value}, hue {hue}: Vibrance reorders chroma {output:?}"
            );
        }
    }
    Ok(())
}
