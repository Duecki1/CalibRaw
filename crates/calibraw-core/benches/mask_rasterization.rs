use calibraw_core::pipeline::{
    rasterize_brush_dabs, BrushDab, MaskGeometry, MaskImage, MaskKind, MaskStack, PathPoint,
};
use std::hint::black_box;
use std::time::Instant;

fn dabs(count: usize, opacity: f32) -> Vec<BrushDab> {
    (0..count)
        .map(|index| {
            let x = 0.08 + 0.84 * (index % 32) as f32 / 31.0;
            let y = 0.08 + 0.84 * (index / 32) as f32 / 7.0;
            BrushDab {
                center: [x, y],
                opacity,
                size: 0.024,
                feather: 0.55,
            }
        })
        .collect()
}

fn measure(label: &str, width: u32, height: u32, dabs: &[BrushDab]) {
    const ITERATIONS: usize = 4;
    let started = Instant::now();
    let mut checksum = 0_u64;
    for _ in 0..ITERATIONS {
        let pixels = rasterize_brush_dabs(width, height, width, height, black_box(dabs));
        checksum = checksum.wrapping_add(
            pixels
                .iter()
                .step_by((pixels.len() / 31).max(1))
                .map(|pixel| u64::from(*pixel))
                .sum(),
        );
        black_box(pixels);
    }
    let elapsed = started.elapsed();
    let megapixels = (width as f64 * height as f64 * ITERATIONS as f64) / 1_000_000.0;
    println!(
        "{label}: {megapixels:.1} MP in {:.3}s ({:.1} MP/s), checksum {checksum}",
        elapsed.as_secs_f64(),
        megapixels / elapsed.as_secs_f64()
    );
}

fn main() {
    let mut args = std::env::args().skip(1);
    let mut sweeps_only = false;
    while let Some(arg) = args.next() {
        if arg == "--sidecar" {
            let path = args.next().expect("--sidecar requires a path");
            measure_sidecar(&path);
            return;
        }
        if arg == "--sweeps" {
            sweeps_only = true;
        }
    }
    if !sweeps_only {
        let positive = dabs(256, 1.0);
        let mixed = dabs(256, -1.0);
        measure("positive brush", 512, 512, &positive);
        measure("erase brush", 512, 512, &mixed);
    }
    let mut generated = generated_mask();
    let mut path = path_mask();
    for edge in [64, 512, 2048] {
        measure_shape_sweeps(
            "generated 6000x4000 source",
            &mut generated,
            0,
            edge,
            6000,
            4000,
        );
        measure_shape_sweeps("curved path", &mut path, 0, edge, 6000, 4000);
    }
}

fn generated_mask() -> MaskStack {
    const WIDTH: u32 = 6000;
    const HEIGHT: u32 = 4000;
    let pixels = (0..HEIGHT)
        .flat_map(|y| {
            let dy = (y as f32 / HEIGHT as f32 - 0.49) / 0.31;
            (0..WIDTH).map(move |x| {
                let dx = (x as f32 / WIDTH as f32 - 0.51) / 0.27;
                ((1.05 - dx * dx - dy * dy) / 0.1 * 255.0).clamp(0.0, 255.0) as u8
            })
        })
        .collect();
    let mut stack = MaskStack::default();
    stack.add_mask(MaskKind::Subject).unwrap();
    stack.masks[0].components[0].geometry = MaskGeometry::Ai {
        mask: MaskImage::new(WIDTH, HEIGHT, pixels),
        grow: 0.15,
        feather: 0.55,
    };
    stack
}

fn path_mask() -> MaskStack {
    let mut stack = MaskStack::default();
    stack.add_mask(MaskKind::Path).unwrap();
    stack.masks[0].components[0].geometry = MaskGeometry::Path {
        points: vec![
            PathPoint {
                position: [0.2, 0.25],
                handle_out: [0.2, -0.14],
                ..PathPoint::corner([0.2, 0.25])
            },
            PathPoint {
                position: [0.77, 0.21],
                handle_in: [-0.15, -0.1],
                handle_out: [0.14, 0.2],
            },
            PathPoint::corner([0.7, 0.52]),
            PathPoint {
                position: [0.8, 0.8],
                handle_in: [0.02, -0.1],
                handle_out: [-0.22, 0.08],
            },
            PathPoint {
                position: [0.18, 0.74],
                handle_in: [0.13, 0.04],
                handle_out: [-0.07, -0.19],
            },
        ],
        grow: 0.15,
        feather: 0.55,
    };
    stack
}

fn set_shape_parameters(stack: &mut MaskStack, layer: usize, grow: f32, feather: f32) {
    for component in &mut stack.masks[layer].components {
        match &mut component.geometry {
            MaskGeometry::Ai {
                grow: current_grow,
                feather: current_feather,
                ..
            }
            | MaskGeometry::Object {
                grow: current_grow,
                feather: current_feather,
                ..
            }
            | MaskGeometry::Path {
                grow: current_grow,
                feather: current_feather,
                ..
            } => {
                *current_grow = grow;
                *current_feather = feather;
            }
            _ => {}
        }
    }
}

fn measure_shape_sweeps(
    label: &str,
    stack: &mut MaskStack,
    layer: usize,
    edge: u32,
    image_width: u32,
    image_height: u32,
) {
    const STEPS: usize = 16;
    const ROUNDS: usize = 2;
    set_shape_parameters(stack, layer, 0.15, 0.55);
    let started = Instant::now();
    black_box(stack.rasterize_layer_f16(layer, edge, edge, image_width, image_height));
    println!(
        "{label} at {edge}x{edge}, first raster: {:.1} ms",
        started.elapsed().as_secs_f64() * 1000.0,
    );
    // Each step changes the slider value while preserving the binary contour.
    // The first raster prepares that contour; timed sweeps measure reuse.
    for sweep_grow in [true, false] {
        let started = Instant::now();
        let mut checksum = 0_u64;
        for _ in 0..ROUNDS {
            for step in 0..STEPS {
                let t = step as f32 / (STEPS - 1) as f32;
                let (grow, feather) = if sweep_grow {
                    (-0.6 + 1.2 * t, 0.55)
                } else {
                    (0.2, t)
                };
                set_shape_parameters(stack, layer, black_box(grow), black_box(feather));
                let pixels =
                    stack.rasterize_layer_f16(layer, edge, edge, image_width, image_height);
                checksum = checksum.wrapping_add(
                    pixels
                        .iter()
                        .step_by((pixels.len() / 31).max(1))
                        .map(|pixel| u64::from(*pixel))
                        .sum(),
                );
                black_box(pixels);
            }
        }
        let elapsed = started.elapsed().as_secs_f64();
        let slider = if sweep_grow { "grow" } else { "feather" };
        println!(
            "{label} at {edge}x{edge}, {slider} sweep: {:.2} ms/raster, checksum {checksum}",
            elapsed * 1000.0 / (STEPS * ROUNDS) as f64,
        );
    }
}

// Read only the edit sidecar; never load the source photograph or display masks.
fn measure_sidecar(path: &str) {
    let bytes = std::fs::read(path).expect("read sidecar");
    let sidecar = calibraw_core::sidecar::decode(&bytes).expect("decode sidecar");
    let masks = &sidecar.edits.masks;
    let (image_width, image_height) = masks
        .masks
        .iter()
        .flat_map(|mask| &mask.components)
        .find_map(|component| match &component.geometry {
            calibraw_core::pipeline::MaskGeometry::Ai {
                mask: Some(mask), ..
            }
            | calibraw_core::pipeline::MaskGeometry::Object {
                mask: Some(mask), ..
            } => Some((mask.width, mask.height)),
            _ => None,
        })
        .unwrap_or((6000, 4000));
    for edge in [64, 512, 2048] {
        for layer in 0..masks.masks.len() {
            let started = Instant::now();
            for _ in 0..3 {
                black_box(masks.rasterize_layer_f16(layer, edge, edge, image_width, image_height));
            }
            println!(
                "mask {} at {edge}x{edge}: {:.1} ms/raster",
                layer + 1,
                started.elapsed().as_secs_f64() * 1000.0 / 3.0
            );
        }
    }
}
