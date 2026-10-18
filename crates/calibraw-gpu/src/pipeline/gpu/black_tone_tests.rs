use super::{direct_shader::run_direct_shader, tests::request_test_device};
use crate::wgpu;

const RAMP_COUNT: usize = 1024;
const PATCH_COUNT: usize = 128;

// Import the actual production module, including its slider response and
// luminance weights; do not duplicate the tone algorithm in the test.
const DIRECT_SHADER: &str = r#"
#import calibraw::tonemap as Tonemap
struct Params { slider: f32, count: u32, _pad0: vec2<u32> }
@group(0) @binding(40) var<uniform> params: Params;
@group(0) @binding(41) var<storage, read> input: array<vec4<f32>>;
@group(0) @binding(42) var<storage, read_write> output: array<vec4<f32>>;
@compute @workgroup_size(64)
fn main(@builtin(global_invocation_id) gid: vec3<u32>) {
    if gid.x >= params.count { return; }
    output[gid.x] = vec4<f32>(
        Tonemap::apply_display_blacks_toe_value(input[gid.x].xyz, params.slider), 1.0);
}
"#;

fn samples() -> Vec<[f32; 4]> {
    let mut values = Vec::with_capacity(RAMP_COUNT + PATCH_COUNT);
    for index in 0..RAMP_COUNT {
        let y = 1.0e-5 * 1.0e5_f32.powf(index as f32 / (RAMP_COUNT - 1) as f32);
        values.push([y, y, y, 1.0]);
    }
    for index in 0..PATCH_COUNT {
        let y = [0.00001, 0.0003, 0.01, 0.035, 0.15, 0.5, 1.0][index % 7];
        let noise = 1.0 + 0.2 * (index as f32 * 0.73).sin();
        values.push([0.6 * y * noise, 0.3 * y * noise, 0.1 * y * noise, 1.0]);
    }
    values.extend([
        [0.0, 0.0, 0.0, 1.0],
        [2.0, 2.0, 2.0, 1.0],
        [4.0, 2.0, 1.0, 1.0],
    ]);
    values
}

fn run_direct(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    input: &[[f32; 4]],
    amount: f32,
) -> anyhow::Result<Vec<[f32; 4]>> {
    run_direct_shader(
        device,
        queue,
        "black-toe direct regression shader",
        "black_tone_tests.wgsl",
        DIRECT_SHADER,
        [amount.to_bits(), input.len() as u32, 0, 0],
        input,
    )
}

fn encoded_luma(rgb: [f32; 4]) -> f32 {
    let value = (0.2627 * rgb[0] + 0.6780 * rgb[1] + 0.0593 * rgb[2]).max(0.0);
    if value <= 0.0031308 {
        12.92 * value
    } else {
        1.055 * value.powf(1.0 / 2.4) - 0.055
    }
}

#[test]
fn negative_black_tone_encoded_slope_is_bounded() -> anyhow::Result<()> {
    let Some((device, queue)) = request_test_device() else {
        eprintln!("black tone regression skipped: no headless wgpu adapter");
        return Ok(());
    };
    let input = samples();
    let baseline = run_direct(&device, &queue, &input, 0.0)?;
    let baseline_slope = (1..RAMP_COUNT)
        .map(|i| {
            (encoded_luma(baseline[i]) - encoded_luma(baseline[i - 1]))
                / (input[i][0] - input[i - 1][0])
        })
        .collect::<Vec<_>>();
    for slider in [-10.0, -25.0, -50.0, -100.0] {
        let output = run_direct(&device, &queue, &input, slider)?;
        let mut maximum: f32 = 0.0;
        let mut shadow_max: f32 = 0.0;
        for i in 1..RAMP_COUNT {
            let slope = (encoded_luma(output[i]) - encoded_luma(output[i - 1]))
                / (input[i][0] - input[i - 1][0])
                / baseline_slope[i - 1];
            maximum = maximum.max(slope);
            if input[i - 1][0] <= 0.15 {
                shadow_max = shadow_max.max(slope);
            }
        }
        eprintln!("slider={slider}: max={maximum:.4}, shadow_max={shadow_max:.4}");
        assert!(
            maximum <= 1.3,
            "slider={slider} relative encoded slope={maximum}"
        );
        assert!(shadow_max <= 1.2, "shadow relative slope={shadow_max}");
    }
    Ok(())
}

#[test]
fn black_tone_direct_invariants_hold() -> anyhow::Result<()> {
    let Some((device, queue)) = request_test_device() else {
        eprintln!("black tone regression skipped: no headless wgpu adapter");
        return Ok(());
    };
    let input = samples();
    let zero = run_direct(&device, &queue, &input, 0.0)?;
    assert_eq!(zero, input);
    for amount in [-100.0, -50.0, -10.0, 10.0, 50.0, 100.0] {
        let output = run_direct(&device, &queue, &input, amount)?;
        assert!(output
            .iter()
            .all(|rgba| rgba.iter().all(|value| value.is_finite())));
        for index in RAMP_COUNT..RAMP_COUNT + PATCH_COUNT {
            let base = input[index];
            let actual = output[index];
            assert!((actual[0] / actual[1] - base[0] / base[1]).abs() <= 1.0e-6);
            assert!((actual[2] / actual[1] - base[2] / base[1]).abs() <= 1.0e-6);
        }
        assert_eq!(output[RAMP_COUNT - 1], input[RAMP_COUNT - 1]);
        assert_eq!(
            &output[RAMP_COUNT + PATCH_COUNT..],
            &input[RAMP_COUNT + PATCH_COUNT..]
        );
        let ramp = &output[..RAMP_COUNT];
        assert!(ramp.windows(2).all(|pair| pair[1][0] >= pair[0][0]));
        for (actual, base) in ramp.iter().zip(&input) {
            assert!(actual[0] >= 0.0);
            if amount < 0.0 {
                assert!(actual[0] <= base[0]);
            } else {
                assert!(actual[0] >= base[0]);
            }
        }
    }
    Ok(())
}

#[test]
fn black_tone_shapes_shadows_and_leaves_middle_grey() -> anyhow::Result<()> {
    let Some((device, queue)) = request_test_device() else {
        eprintln!("black tone regression skipped: no headless wgpu adapter");
        return Ok(());
    };
    // Display-linear luminance: deep shadow, shadow and display middle grey.
    let levels = [0.01_f32, 0.05, 0.1845];
    let input = levels.map(|y| [y, y, y, 1.0]);
    let change_ev = |slider: f32| -> anyhow::Result<[f32; 3]> {
        let output = run_direct(&device, &queue, &input, slider)?;
        Ok(std::array::from_fn(|index| {
            (output[index][1] / input[index][1]).log2()
        }))
    };

    let crush = change_ev(-100.0)?;
    let lift = change_ev(100.0)?;
    eprintln!("Blacks -100: {crush:?} EV, +100: {lift:?} EV");
    for (name, change) in [("-100", crush), ("+100", lift)] {
        assert!(
            change[2].abs() <= 0.1,
            "Blacks {name} moved middle grey by {} EV",
            change[2]
        );
        assert!(
            change[0].abs() > change[1].abs() && change[1].abs() > change[2].abs(),
            "Blacks {name} does not fade from the deepest shadows: {change:?}"
        );
    }
    assert!(crush[0] <= -1.2, "Blacks -100 at Y=0.01: {} EV", crush[0]);
    assert!(
        (-1.0..=-0.3).contains(&crush[1]),
        "Blacks -100 at Y=0.05: {} EV",
        crush[1]
    );
    assert!(lift[0] >= 1.0, "Blacks +100 at Y=0.01: {} EV", lift[0]);
    assert!(
        (0.3..=1.0).contains(&lift[1]),
        "Blacks +100 at Y=0.05: {} EV",
        lift[1]
    );
    Ok(())
}
