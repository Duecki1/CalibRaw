// Rain: falling drops, the mist of unresolved drops beyond them and the water they
// throw up where they land.
//
// A drop is transparent and refracts about 165 degrees of its surroundings, so
// its brightness hardly depends on what lies behind it (Garg and Nayar,
// "Photometric Model of a Rain Drop", 2003). A streak therefore replaces a share
// of the background with the light around the drop, rather than adding light: it
// shows light against a dark wall and dark against a bright sky.
//
// Drops are photographed by the photo's own camera. During the exposure a drop of
// diameter w falls a distance L, and the lens spreads it into a disc of radius R.
// What the drop covers stays the same, so at the middle of its streak it replaces a
// share (w / 2R) (w / max(L, 2R)) of the background: long and blurred streaks are
// fainter. L follows the shutter speed (RainEffectSettings::streak_length). The blur
// at each distance is measured in the photo (the scene bake's lens blur), so drops are as soft
// as what lies at their distance; without scene depth a control sets the focus.
//
// Drops fill the air evenly, so the image holds many small far ones and few large
// near ones. The air is cut into slabs, and each slab into a grid of cells holding
// at most one streak each, placed by hashing the cell. Every streak draws its own
// distance within the slab and its own size. Cells stretch with the streak length
// and a cell's hash decides whether it holds a drop, so changing a control moves or
// fades the same streaks rather than reshuffling them. Sizes are in shorter-edge
// units.
//
// A pixel visits only the cells around its own, so a streak has to fit within them.
// A strongly blurred streak does not: its cells merge by powers of two, and a merged
// cell draws one of the streaks it stands for, so every level draws a subset of the
// same streaks. A sum of n blurred streaks varies by sqrt(n) of one of them, which is
// what the one drawn carries. The rest of their light is the even veil Mist shows.

// Packed parameter lanes and options: `RAIN_<NAME>_LANE`, `RAIN_DEPTH_OPTION`
// (generated from the effect registry, effect_wgsl.rs).

const RAIN_SLABS: u32 = 6u;
// The sides of the slabs, far to near, as Relight's light depth: 0 at the nearest
// surface and 1 at the farthest, negative in the open air before the nearest surface
// down to -1 at the camera.
const RAIN_SLAB_SIDES: array<f32, 7> = array<f32, 7>(1.0, 0.62, 0.28, 0.0, -0.4, -0.7, -0.92);
// Streaks per square shorter edge at full Amount, per unit of cubed distance: the
// volume of air behind a piece of the image.
const RAIN_DROPS_PER_VOLUME: f32 = 700.0;
// Streak width of a typical drop at the nearest surface, in shorter edges, at the
// lowest and highest Drop size.
const RAIN_WIDTH_RANGE: vec2<f32> = vec2<f32>(0.0003, 0.0018);
// The streak length, in shorter edges at the nearest surface, a slab's cells are made
// for, and the share of them that may hold a drop: full cells would space their
// streaks evenly, as a comb. Past that share streaks only grow longer.
const RAIN_DENSE_LENGTH: f32 = 0.1;
const RAIN_MOST_CELLS: f32 = 0.4;
// Longest streak in shorter edges, for the nearest slabs.
const RAIN_MAX_LENGTH: f32 = 1.4;
// Drop diameters relative to a typical drop: an exponential distribution, as rain's
// (Marshall and Palmer, "The Distribution of Raindrops with Size", 1948), of many
// small drops and few large ones.
const RAIN_DROP_SMALLEST: f32 = 0.35;
const RAIN_DROP_SPREAD: f32 = 0.65;
const RAIN_DROP_LARGEST: f32 = 2.0;
// How a typical drop's diameter grows with the rain rate (Marshall and Palmer).
const RAIN_DROP_RATE_EXPONENT: f32 = 0.21;
// The fall speed of the largest drop relative to a typical drop's (rain_drop_speed).
const RAIN_DROP_FASTEST: f32 = 1.46;
// Share of the background a streak replaces, relative to w / max(L, 2R). Well above
// 1: the photo shows less of the sky than a drop sees, so `around_drop` is too dim to
// show streaks at their true share.
const RAIN_OPACITY: f32 = 40.0;
const RAIN_MAX_OPACITY: f32 = 0.85;
// A drop's oscillation beads its streak: how strongly when it is sharp, and the
// distance from one bead to the next in drop widths, about one period at 6 m/s.
const RAIN_BEADING: f32 = 0.3;
const RAIN_BEAD_SPACING: f32 = 12.0;
// Largest slope of a single streak against the others. Drops do not fall exactly
// parallel, but a steeper streak would leave the cells around it.
const RAIN_DROP_TILT: f32 = 0.03;
// Radius in pixels up to which a streak is soft all the way across.
const RAIN_SOFT_RADIUS: f32 = 4.0;
// Share of a streak's length over which its ends fade at least.
const RAIN_END_TAPER: f32 = 0.2;
// Share of a cell's width a streak's radius may take; its tilt has the rest.
const RAIN_CELL_RADIUS: f32 = 0.75;
// Largest slope of a slab's rain against the fall direction, and how far the air
// shifts it across the frame in shorter edges: rain is not one sheet.
const RAIN_SLAB_TILT: f32 = 0.035;
const RAIN_DRIFT: f32 = 0.018;
// Without scene depth: the defocus radius in shorter edges per unit of difference in
// inverse distance.
const RAIN_DEFOCUS: f32 = 0.0016;
// With it: the least the blur grows before the nearest surface, per unit of inverse
// distance. The scene's depth range says little about a telephoto lens, whose nearby
// drops melt away although all of the scene is sharp.
const RAIN_NEAR_BLUR_RATE: f32 = 0.003;
// Share of the depth range over which a drop passes behind a surface.
const RAIN_OCCLUSION_SOFTNESS: f32 = 0.03;
// Forward scattering of drops: rain shows best looking toward a light.
const RAIN_LIGHT_ANISOTROPY: f32 = 0.72;
// A drop's light relative to the light it sees, what its refraction passes on (Garg
// and Nayar), and how far up and down the fall it looks for it, in shorter edges:
// half of what it sees lies above it, usually the brightest part of the scene.
const RAIN_DROP_TRANSMISSION: f32 = 0.94;
const RAIN_DROP_VIEW: f32 = 1.0;
// Light a drop refracts from Relight and Light Rays lights at Light glow 100.
const RAIN_LIGHT_GAIN: f32 = 2.4;
// Image light a drop refracts at Light glow 100, relative to the halo's mean.
const RAIN_IMAGE_LIGHT_GAIN: f32 = 40.0;
// Mist optical depth at the farthest surface at full Amount and Mist.
const RAIN_MIST_DEPTH: f32 = 1.1;
// Mist before the nearest surface, relative to the depth range.
const RAIN_MIST_NEAR: f32 = 0.12;
const RAIN_MIST_GAIN: f32 = 1.1;
const RAIN_MIST_GLOW_GAIN: f32 = 4.0;
// How far gusts bend the rain, in shorter edges.
const RAIN_GUST_BEND: f32 = 0.05;
// A splash's radius and its cell in the plane of rain_splashes, where they keep
// their size: a radius of about 1.5 cm for a camera about 1.5 m above the ground.
const RAIN_SPLASH_RADIUS: f32 = 0.008;
const RAIN_SPLASH_CELL: f32 = 0.03;
// Drops one splash throws.
const RAIN_SPLASH_DROPS: u32 = 3u;
// Splashes per square camera height of ground at full Amount.
const RAIN_SPLASH_DENSITY: f32 = 60.0;
const RAIN_SPLASH_OPACITY: f32 = 1.6;
// The streak length up to which an exposure holds a splash whole. A splash lasts
// about 50 ms, so a longer exposure sees it for a smaller share of its time.
const RAIN_SPLASH_LENGTH: f32 = 0.15;

// How much the lens blurs each distance.
struct RainLens {
    // Whether the photo says: see `rain_lens_blur`.
    measured: bool,
    // Measured: the blur at the nearest surface, and how much it grows nearer the
    // camera per unit of inverse distance.
    nearest: f32,
    rate: f32,
    // Not measured: the inverse distance in focus.
    focus: f32,
}

// The frame rain's slabs live in: the depth result's own with a scene bake,
// else the plane it assumes (a 30 mm lens, about the relief of Relight's default).
const RAIN_FLAT_FOCAL_TAN: f32 = 0.4;
const RAIN_FLAT_RELIEF: f32 = 2.283;

fn rain_camera(baked: bool) -> Camera::SceneCamera {
    if baked {
        return Bake::scene_bake.camera;
    }
    return Camera::SceneCamera(
        vec4<f32>(RAIN_FLAT_FOCAL_TAN, 1.0, 1.0, RAIN_FLAT_RELIEF), vec4<f32>(1.0, 0.0, 1.0, 1.0),
    );
}

// The frame depth of a slab side or drop: before the nearest surface, down to
// -1 at the camera, depth is a share of the way to it.
fn rain_depth_z(camera: Camera::SceneCamera, depth: f32) -> f32 {
    if depth < 0.0 { return 1.0 + 0.9 * depth; }
    return Camera::scene_frame_depth(camera, depth, 1.0);
}

// The lens blur at linear depth `depth` as a Gaussian's deviation in shorter
// edges: the scene bake's fit to the photo's own edges (scene/bake_focus.wgsl).
// Only called with a scene bake.
fn rain_scene_blur(depth: f32) -> f32 {
    let size = Bake::scene_bake_size();
    return Bake::scene_lens_blur(Bake::scene_bake_frame_depth(depth)) / f32(min(size.x, size.y));
}

// A thin lens blurs in proportion to |1 / distance - 1 / focus|, so what the farthest
// and the nearest surface blur together is that rate across their inverse distances
// when the focus lies between them or at either.
fn rain_lens(measured: bool, camera: Camera::SceneCamera, focus_control: f32) -> RainLens {
    if !measured {
        return RainLens(false, 0.0, 0.0, 1.0 / mix(0.3, 3.0, focus_control * focus_control));
    }
    let nearest = rain_scene_blur(0.0);
    let span = 1.0 - 1.0 / rain_depth_z(camera, 1.0);
    let rate = max((nearest + rain_scene_blur(1.0)) / span, RAIN_NEAR_BLUR_RATE);
    return RainLens(true, nearest, rate, 0.0);
}

// The lens blur at `depth`, which lies at `distance`, as a Gaussian's deviation in
// shorter edges. The photo holds nothing before its nearest surface to measure.
fn rain_lens_blur(lens: RainLens, depth: f32, distance: f32) -> f32 {
    let inverse = 1.0 / distance;
    if !lens.measured { return abs(inverse - lens.focus) * RAIN_DEFOCUS; }
    if depth >= 0.0 { return rain_scene_blur(depth); }
    return lens.nearest + lens.rate * (inverse - 1.0);
}

// Fall speed relative to a typical drop's, of 1.5 mm: 9.65 - 10.3 exp(-0.6 D) m/s
// (Atlas, Srivastava and Sekhon, "Doppler radar characteristics of precipitation at
// vertical incidence", 1973). Larger drops fall faster, so their streaks are longer.
fn rain_drop_speed(diameter: f32) -> f32 {
    return (9.65 - 10.3 * exp(-0.9 * diameter)) / 5.46;
}

// Everything about one Rain effect that is the same for every slab at a pixel.
struct RainField {
    // The pixel in the warped plane where streaks are straight and parallel:
    // x across the fall, y along it.
    warped: vec2<f32>,
    // Image points per warped unit at this pixel.
    scale: f32,
    // A typical streak's width and length at the nearest surface.
    width: f32,
    length: f32,
    amount: f32,
    seed: u32,
    gusts: f32,
    // Two independent fields each slab mixes its own curtains and drift from, so
    // that slabs do not share one pattern: see `rain_slab_mix`.
    curtains: vec2<f32>,
    drift: vec2<f32>,
    camera: Camera::SceneCamera,
    lens: RainLens,
    // Whether drops pass behind surfaces, and the depth of the surface here.
    occluded: bool,
    surface_depth: f32,
    // Whether depths are the scene bake's, and slab sides are control depths.
    baked: bool,
}

// A slab side from `RAIN_SLAB_SIDES` as linear depth: sides at or past the
// nearest surface are control depths c (even in the log of distance), so
// slabs spread over the scene whatever its range; those before it are as they
// are.
fn rain_side(field: RainField, side: f32) -> f32 {
    if side <= 0.0 || !field.baked { return side; }
    return Camera::scene_depth_of_control(field.camera, side);
}

// A slab's grid and what the blur does to its streaks.
struct RainSlab {
    // Distances of its sides, and the lens blur there.
    near: f32,
    far: f32,
    near_blur: f32,
    far_blur: f32,
    // A cell before merging, and how many of them a merged cell stands for.
    cell: vec2<f32>,
    merged: vec2<i32>,
    // The share of cells before merging that hold a drop.
    presence: f32,
    // The largest radius with which a streak stays in the cells around its own.
    max_radius: f32,
}

// The power of two that `ratio` needs to reach 1.
fn rain_merge(ratio: f32) -> i32 {
    return 1 << u32(ceil(log2(max(ratio, 1.0))));
}

// None of this depends on the Drop size control or on the pixel size, so that those
// keep the same streaks.
fn rain_slab(field: RainField, layer: u32) -> RainSlab {
    var sides = RAIN_SLAB_SIDES;
    let far_depth = rain_side(field, sides[layer]);
    let near_depth = rain_side(field, sides[layer + 1u]);
    let near = rain_depth_z(field.camera, near_depth);
    let far = rain_depth_z(field.camera, far_depth);
    let near_blur = rain_lens_blur(field.lens, near_depth, near);
    let far_blur = rain_lens_blur(field.lens, far_depth, far);

    let size = 1.0 / near;
    let dense = min(RAIN_DENSE_LENGTH * size, RAIN_MAX_LENGTH);
    let longest = min(field.length * size, RAIN_MAX_LENGTH) * RAIN_DROP_FASTEST;
    let widest = RAIN_WIDTH_RANGE.y * size * RAIN_DROP_LARGEST;
    let density = RAIN_DROPS_PER_VOLUME * (far * far * far - near * near * near);
    let cell = vec2<f32>(1.0 / (density * dense), longest + widest);
    let blur = max(near_blur, far_blur);
    let radius = sqrt(0.25 * widest * widest + 4.0 * blur * blur);
    let merged = vec2<i32>(
        rain_merge(radius / (RAIN_CELL_RADIUS * cell.x)),
        rain_merge((longest + 2.0 * radius) / cell.y),
    );
    let room = vec2<f32>(merged) * cell;
    return RainSlab(
        near,
        far,
        near_blur,
        far_blur,
        cell,
        merged,
        field.amount * min(cell.y / dense, RAIN_MOST_CELLS),
        min(RAIN_CELL_RADIUS * room.x, 0.5 * (room.y - longest)),
    );
}

// The share of the background a streak replaces at `offset` from its start.
fn rain_streak(field: RainField, slab: RainSlab, offset: vec2<f32>, random: vec4<f32>) -> f32 {
    // The air behind a piece of the image widens with distance, so more of a slab's
    // drops lie on its far side.
    let cubed = mix(slab.near * slab.near * slab.near, slab.far * slab.far * slab.far, random.x);
    let distance = pow(cubed, 1.0 / 3.0);
    var visible = 1.0;
    if field.occluded && distance > 1.0 {
        let depth = (distance - 1.0) / field.camera.lens.w;
        visible = smoothstep(
            depth - RAIN_OCCLUSION_SOFTNESS,
            depth + RAIN_OCCLUSION_SOFTNESS,
            field.surface_depth,
        );
        if visible <= 1e-4 { return 0.0; }
    }

    let size = 1.0 / distance;
    let diameter = min(
        RAIN_DROP_SMALLEST - RAIN_DROP_SPREAD * log(max(1.0 - random.y, 1e-4)),
        RAIN_DROP_LARGEST,
    );
    let width = field.width * size * diameter;
    let length = min(field.length * size, RAIN_MAX_LENGTH) * rain_drop_speed(diameter);
    let share = (distance - slab.near) / (slab.far - slab.near);
    let blur = mix(slab.near_blur, slab.far_blur, share);
    let pixel = scene_pixel_extent() / field.scale;
    // A disc of radius R has the deviation of a Gaussian of R / 2.
    let radius = min(
        sqrt(0.25 * width * width + 4.0 * blur * blur + pixel * pixel),
        slab.max_radius,
    );
    let extent = length + 2.0 * radius;
    if offset.y >= extent { return 0.0; }
    let slope = (random.z - 0.5) * min(RAIN_DROP_TILT, (1.0 - RAIN_CELL_RADIUS) * 2.0
        * f32(slab.merged.x) * slab.cell.x / extent);
    let across = (offset.x - offset.y * slope) / radius;
    if abs(across) >= 1.0 { return 0.0; }

    // Across the streak: the chords of the disc it sweeps. A disc only a few pixels
    // wide has no rim to show: the lens and the sensor soften it, and a rim would alias.
    let softness = clamp(RAIN_SOFT_RADIUS * pixel / radius, 0.0, 1.0);
    let profile = pow(1.0 - across * across, mix(0.5, 2.0, softness));
    // The profile's area, relative to the radius, at those two exponents.
    let area = mix(1.571, 1.067, softness);
    let sweep = min(0.785 * width / (radius * area), 1.0);
    // Along it: the disc enters and leaves over the shorter of its diameter and its
    // fall, and a drop's light also changes over the fall.
    let rise = min(length, max(2.0 * radius, RAIN_END_TAPER * length));
    let ends = smoothstep(0.0, rise, offset.y) * (1.0 - smoothstep(extent - rise, extent, offset.y));
    let opacity = min(RAIN_OPACITY * width / max(length, 2.0 * radius), RAIN_MAX_OPACITY);
    let beats = offset.y / (RAIN_BEAD_SPACING * width);
    let wobble = 0.6 * sin(6.283 * (beats + random.w)) + 0.4 * sin(8.61 * beats + 20.0 * random.w);
    let beads = 1.0 + RAIN_BEADING * wobble * min(0.5 * width / radius, 1.0);
    return opacity * sweep * ends * beads * profile * visible;
}

// A unit vector of the slab's own, which mixes it its share of two fields.
fn rain_slab_mix(field: RainField, layer: u32) -> vec2<f32> {
    let angle = 6.2831853 * mask_effect_hash_unit(field.seed * 2097593u + layer * 104729u + 3u);
    return vec2<f32>(cos(angle), sin(angle));
}

// Coverage of the streaks of a slab at a pixel: the sum over the cells whose streak
// can reach it of the share of the background they replace.
fn rain_slab_coverage(field: RainField, layer: u32) -> f32 {
    let slab = rain_slab(field, layer);
    let salt = field.seed * 1048583u + layer * 7919u;
    let slab_mix = rain_slab_mix(field, layer);
    let tilt = RAIN_SLAB_TILT * (2.0 * mask_effect_hash_unit(salt + 5u) - 1.0);
    let bend = RAIN_DRIFT + field.gusts * RAIN_GUST_BEND;
    let warped = vec2<f32>(
        field.warped.x + field.warped.y * tilt + bend * dot(field.drift, slab_mix),
        field.warped.y,
    );
    let curtain = 0.5 + dot(field.curtains, slab_mix);
    let strength = mix(1.0, smoothstep(0.28, 0.72, curtain) * 1.7, field.gusts);
    if strength <= 1e-4 { return 0.0; }

    let merged = vec2<f32>(slab.merged);
    let room = merged * slab.cell;
    let cell = vec2<i32>(floor(warped / room));
    let is_merged = any(slab.merged > vec2<i32>(1));
    var coverage = 0.0;
    for (var dy = -1; dy <= 0; dy = dy + 1) {
        for (var dx = -1; dx <= 1; dx = dx + 1) {
            let candidate = cell + vec2<i32>(dx, dy);
            var drawn = candidate * slab.merged;
            if is_merged {
                let pick = effect_cell_random(candidate, salt + 2u).xy * 0.9999;
                drawn += vec2<i32>(pick * merged);
            }
            let placed = effect_cell_random(drawn, salt);
            if placed.x >= slab.presence { continue; }
            let offset = warped - (vec2<f32>(drawn) + placed.yz) * slab.cell;
            if offset.y < 0.0 || offset.y >= room.y || abs(offset.x) >= room.x { continue; }
            coverage += rain_streak(field, slab, offset, effect_cell_random(drawn, salt + 1u));
        }
    }
    return min(coverage * sqrt(merged.x * merged.y) * strength, 1.0);
}

// What one splash leaves at `offset` from where it landed, up the image positive, in
// the plane of rain_splashes: the raised rim of its crown, seen at `aspect` as a ring
// on the ground is, and the drops it throws. A young splash is small and tall, and
// settles into a wider, lower and fainter ring. `blur` is the lens's disc there: each
// part spreads with it, and fades as its light spreads.
fn rain_splash_shape(
    offset: vec2<f32>,
    radius: f32,
    blur: f32,
    aspect: f32,
    random: vec4<f32>,
) -> f32 {
    let age = random.x;
    let rim_radius = radius * mix(0.35, 1.0, age);
    let rim = (offset - vec2<f32>(0.0, radius * mix(1.0, 0.15, age))) / vec2<f32>(1.0, aspect);
    let rim_width = 0.35 * rim_radius;
    let spread = sqrt(rim_width * rim_width + blur * blur);
    let along_rim = (length(rim) - rim_radius) / spread;
    // A crown is never even: one side stands higher.
    let uneven = 0.6 + 0.4 * sin(atan2(rim.y, rim.x) + 6.283 * random.y);
    let fading = 1.0 - 0.7 * age;
    var shape = exp(-along_rim * along_rim) * uneven * fading * rim_width / spread;

    let drop = 0.16 * radius;
    let drop_spread = drop * drop + blur * blur;
    for (var index = 0u; index < RAIN_SPLASH_DROPS; index = index + 1u) {
        let thrown = fract(random * (f32(index) + 3.17) * 7.31);
        let place = vec2<f32>(
            (thrown.x - 0.5) * 2.4 * radius,
            (1.0 + 1.6 * thrown.y) * radius,
        ) - offset;
        shape += exp(-dot(place, place) / drop_spread) * (thrown.z * fading) * drop * drop / drop_spread;
    }
    return shape;
}

// Coverage of splashes at a ground pixel. They are placed in a plane where they keep
// their size and shape, so that they stand up from the ground in the image: across
// the view and up it, both relative to the distance below the horizon, which is what
// a thing's image scales with. A row of its cells lies at one distance. Rows farther
// away hold more ground, and so more splashes, until their cells are full.
fn rain_splashes(field: RainField, pixel: WetPixel, control: f32) -> f32 {
    let pixel_below = Geometry::scene_below_horizon(pixel.point);
    if pixel_below <= 1e-3 { return 0.0; }
    // Camera heights per image point below the horizon, for a level camera.
    let focal = Bake::scene_bake.camera.lens.x;
    let ground_scale = 0.5 / focal;
    let right = vec2<f32>(-pixel.up.y, pixel.up.x);
    let lateral = dot(pixel.point, right);
    let row = i32(floor(log(pixel_below) / RAIN_SPLASH_CELL));
    let exposure = min(RAIN_SPLASH_LENGTH / field.length, 1.0);
    let salt = field.seed * 3145739u + 41u;
    var coverage = 0.0;
    for (var dy = -1; dy <= 1; dy = dy + 1) {
        let candidate_row = row + dy;
        let row_below = exp((f32(candidate_row) + 0.5) * RAIN_SPLASH_CELL);
        let ground_distance = ground_scale / row_below;
        let cell_ground = ground_scale * ground_distance * RAIN_SPLASH_CELL * RAIN_SPLASH_CELL;
        let presence = RAIN_SPLASH_DENSITY * cell_ground * field.amount;
        let across = lateral / row_below;
        let column = i32(floor(across / RAIN_SPLASH_CELL));
        for (var dx = -1; dx <= 1; dx = dx + 1) {
            let cell = vec2<i32>(column + dx, candidate_row);
            let placed = effect_cell_random(cell, salt);
            if placed.x >= presence { continue; }
            let below = row_below * exp((placed.z - 0.5) * RAIN_SPLASH_CELL);
            let offset = vec2<f32>(
                across - (f32(cell.x) + placed.y) * RAIN_SPLASH_CELL,
                (below - pixel_below) / row_below,
            );
            let random = effect_cell_random(cell, salt + 1u);
            let radius = RAIN_SPLASH_RADIUS * mix(0.5, 1.3, placed.w);
            let blur = sqrt(4.0 * pixel.blur * pixel.blur
                + scene_pixel_extent() * scene_pixel_extent()) / row_below;
            // The ground's tilt toward the view there.
            let slope = 2.0 * focal * row_below;
            let aspect = slope / sqrt(1.0 + slope * slope);
            coverage += rain_splash_shape(offset, radius, blur, aspect, random);
        }
    }
    return min(coverage * control * exposure * RAIN_SPLASH_OPACITY, 1.0);
}

// What a drop refracts toward the camera of the light it receives, `received`
// (relative to the ambient level, from scene_lights.wgsl): scene lights and
// emission, and the photograph's own light sources with Image lights.
fn rain_drop_light(pos: vec2<i32>, params: MaskEffectParams, received: vec3<f32>) -> vec3<f32> {
    let glow = mask_effect_percent(params, RAIN_LIGHT_GLOW_LANE);
    if glow <= 1e-6 { return vec3<f32>(0.0); }
    var result = received * (glow * RAIN_LIGHT_GAIN * scene_ambient_level());
    if medium_image_lights_enabled(params) {
        result += image_light_at(pos) * (glow * RAIN_IMAGE_LIGHT_GAIN);
    }
    return result;
}

// Rain over one surface of a pixel, at linear depth `surface_depth`: a pixel with
// two surfaces is rained on twice and blended by coverage, so a drop between
// the near and the far surface shows through the part the near one does not
// cover (`apply_rain`).
fn rain_shade(
    pos: vec2<i32>,
    input_rgb: vec3<f32>,
    params: MaskEffectParams,
    surface_depth: f32,
    uses_depth: bool,
    receiver: MediumTarget,
) -> vec3<f32> {
    let amount = mask_effect_percent(params, RAIN_AMOUNT_LANE);
    let uv = full_image_uv(pos);
    let camera = rain_camera(receiver.baked);

    let seed = u32(clamp(mask_effect_lane(params, RAIN_SEED_LANE), 0.0, 1000.0));
    let seed_offset = vec2<f32>(f32(seed) * 0.071 + 5.3, f32(seed) * -0.113 + 41.7);
    let image_size = Common::scene_image_size();
    let center = 0.5 * image_size;
    let point = uv * image_size;
    let fall = mask_effect_lane_pair(params, RAIN_FALL_X_LANE);
    let across = vec2<f32>(fall.y, -fall.x);
    let relative = point - center;
    let plane = vec2<f32>(dot(relative, across), dot(relative, fall));
    // A homography: straight lines stay straight, and streaks parallel in the
    // warped plane meet at a vanishing point on the fall axis.
    let perspective = clamp(mask_effect_lane(params, RAIN_PERSPECTIVE_LANE) / 100.0, -1.0, 1.0);
    let denominator = max(1.0 + perspective * 0.7 * plane.y / max(image_size.x, image_size.y), 0.15);
    let warped = plane / denominator;

    // The light around the drops. Light reaches the mist from above as well.
    let around = 0.6 * scene_environment_blurred_at(uv)
        + 0.4 * scene_environment_blurred_at(vec2<f32>(uv.x, uv.y * 0.3));
    let view = fall * RAIN_DROP_VIEW / image_size;
    let around_drop = (0.5 * scene_environment_blurred_at(uv - view)
        + 0.25 * scene_environment_blurred_at(uv)
        + 0.25 * scene_environment_blurred_at(uv + view)) * RAIN_DROP_TRANSMISSION;

    // Gusts: the rain gathers into uneven curtains and bends along them.
    let gusts = mask_effect_percent(params, RAIN_GUSTS_LANE);
    let curtain_point = vec2<f32>(warped.x * 2.1, warped.y * 0.55) + seed_offset;
    var curtains = vec2<f32>(0.0);
    if gusts > 1e-4 {
        curtains = vec2<f32>(
            effect_fbm(curtain_point),
            effect_fbm(curtain_point * 1.3 + vec2<f32>(31.7, -12.4)),
        ) - vec2<f32>(0.5);
    }
    let drift_point = curtain_point * 0.7;
    let drift = vec2<f32>(
        effect_noise(drift_point + vec2<f32>(9.2, 3.1)),
        effect_noise(drift_point + vec2<f32>(-17.3, 22.6)),
    ) - vec2<f32>(0.5);
    let strength = mix(1.0, smoothstep(0.28, 0.72, 0.5 + curtains.x) * 1.7, gusts);

    let size = mask_effect_percent(params, RAIN_DROP_SIZE_LANE);
    let focus_control = mask_effect_percent(params, RAIN_FOCUS_LANE);
    let field = RainField(
        warped,
        denominator,
        mix(RAIN_WIDTH_RANGE.x, RAIN_WIDTH_RANGE.y, size * size) * pow(amount, RAIN_DROP_RATE_EXPONENT),
        mask_effect_lane(params, RAIN_LENGTH_LANE),
        pow(amount, 1.2),
        seed,
        gusts,
        curtains,
        drift,
        camera,
        rain_lens(uses_depth, camera, focus_control),
        uses_depth,
        surface_depth,
        receiver.baked,
    );

    // What lands on the ground, seen through the mist like the ground itself.
    var ground_rgb = input_rgb;
    let splashes = mask_effect_percent(params, RAIN_SPLASHES_LANE);
    if uses_depth && receiver.baked && splashes > 1e-4 && Geometry::scene_has_ground() {
        let pixel = wet_pixel(pos, uv, receiver.surface);
        // Only what is surely ground: a wall near the horizon may pass the plane's tests.
        let on_ground = smoothstep(0.4, 0.9, pixel.on_ground);
        if on_ground > 1e-4 {
            var received = vec3<f32>(0.0);
            if mask_effect_percent(params, RAIN_LIGHT_GLOW_LANE) > 1e-6 {
                received = scene_light_on_surface(pos, uv, receiver.pixel, receiver.surface, RAIN_LIGHT_ANISOTROPY);
            }
            let drop = around_drop + rain_drop_light(pos, params, received);
            ground_rgb = mix(ground_rgb, drop, rain_splashes(field, pixel, splashes) * on_ground);
        }
    }

    // The mist of drops too small and many to resolve: extinction grows with
    // the rain rate as R^0.67 (Tremblay et al., "Rain Rendering for Evaluating
    // and Improving Robustness to Bad Weather", 2021), and it scatters scene
    // lights as Fog does.
    let mist = mask_effect_percent(params, RAIN_MIST_LANE);
    let extinction = RAIN_MIST_DEPTH * mist * pow(amount, 0.67) * mix(1.0, strength, 0.5);
    let distance = max(surface_depth, 0.0);
    let optical_depth = extinction * (RAIN_MIST_NEAR + distance);
    let transmission = exp(-optical_depth);
    var rgb = ground_rgb * transmission + around * RAIN_MIST_GAIN * (1.0 - transmission);
    let glow = mask_effect_percent(params, RAIN_LIGHT_GLOW_LANE);
    if extinction > 1e-6 && glow > 1e-6 {
        let ray = vec3<f32>(effect_image_point(pos) * 1.25, 1.0);
        let volume = MediumVolume(ray, 0.0, 0.1, 1.0, vec3<f32>(0.0), 0.0, 0.5);
        let scattered = medium_light_scattering(pos, volume, distance, extinction, receiver);
        rgb += scattered * (glow * RAIN_MIST_GLOW_GAIN * scene_ambient_level());
        rgb += medium_image_light(pos, params, glow, 1.0, 1.0 - transmission);
    }

    // Streaks, far to near, each slab veiled by the mist in front of it.
    var sides = RAIN_SLAB_SIDES;
    for (var layer = 0u; layer < RAIN_SLABS; layer = layer + 1u) {
        let near_depth = rain_side(field, sides[layer + 1u]);
        // A surface before the whole slab hides all of it.
        if uses_depth && near_depth > 0.0
            && surface_depth <= near_depth - RAIN_OCCLUSION_SOFTNESS {
            continue;
        }
        let coverage = rain_slab_coverage(field, layer);
        if coverage <= 1e-4 { continue; }
        let depth = 0.5 * (near_depth + rain_side(field, sides[layer]));
        let veil = exp(-extinction * (RAIN_MIST_NEAR + max(depth, 0.0)));
        // Drops scatter forward, so a light straight behind them shows most:
        // surfaces between them hide it (the light bake's cubes), unless the
        // rain ignores them.
        var received = vec3<f32>(0.0);
        if glow > 1e-6 {
            received = scene_light_in_air(pos, uv, depth, RAIN_LIGHT_ANISOTROPY);
        }
        let drop = around_drop + rain_drop_light(pos, params, received);
        rgb = mix(rgb, drop, coverage * veil);
    }
    return rgb;
}

fn apply_rain(pos: vec2<i32>, input_rgb: vec3<f32>, params: MaskEffectParams) -> vec3<f32> {
    if mask_effect_percent(params, RAIN_AMOUNT_LANE) <= 1e-6 { return input_rgb; }
    let wants_depth = params.options[RAIN_DEPTH_OPTION] > 0.5;
    let uv = full_image_uv(pos);
    if wants_depth && Geometry::scene_geometry_present() {
        let scene_rgb = Geometry::scene_unedited_at(pos);
        let pixel = Geometry::scene_pixel(uv, scene_rgb, 1.0);
        let colors = Geometry::scene_split(pixel, scene_rgb, input_rgb);
        let near = rain_shade(
            pos, colors[0], params, medium_surface_depth(pixel.near), true,
            medium_target(pixel, pixel.near, uv),
        );
        if !pixel.in_band {
            return near;
        }
        let far = rain_shade(
            pos, colors[1], params, medium_surface_depth(pixel.far), true,
            medium_target(pixel, pixel.far, uv),
        );
        return Geometry::scene_blend(pixel, near, far);
    }
    // Without scene depth, or without the effect reading it, the scene lies at an
    // assumed distance; rain without depth falls in front of everything.
    let surface_depth = select(1.0, SCENE_DEPTH_UNKNOWN, wants_depth);
    return rain_shade(pos, input_rgb, params, surface_depth, false, medium_unbaked(uv));
}
