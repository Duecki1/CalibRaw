// Lens Blur (defocus.rs in calibraw-core sets what the controls mean and
// packs them; pipeline/gpu/defocus.rs runs the passes). Colour is
// scene-linear Rec.2020, the first Blur stage's input, which may be negative
// or above 1.
//
// Model. Every surface of a pixel (scene/geometry.wgsl: up to two, with the
// near one's coverage α as their matte) is a light source drawn as a disc of
// the aperture's shape, of radius c, its circle of confusion: 0 inside the
// focus range, growing linearly in inverse distance away from it, signed
// negative in front of it (`defocus_coc`). Without depth every masked pixel
// takes the largest radius; unmasked pixels stay sharp (c = 0). Radii are in
// the full image's shorter edges, so the look does not depend on resolution.
//
// Slabs. Surfaces are sorted by |c| into the focus slab (c < ρ0) and six
// bins of an octave each, [ρ0·2^b, ρ0·2^(b+1)), in front of the range and
// behind it, with a soft edge of DEFOCUS_BIN_SOFTNESS octaves so a surface
// crossing an edge leaves no seam. Slabs are ordered by depth: front bins
// from the widest, the focus slab, back bins from the narrowest (c grows
// monotonically with distance on either side). Following the layered
// renderers (Wadhwa et al. 2018; Sheng et al., Dr.Bokeh, 2024), each slab is
// blurred on its own and the slabs are composited front to back with "over",
// so a nearer slab covers what lies behind it: a blurred foreground spreads
// over the scene behind, and a sharp subject keeps its edge.
//
// A slab is a scatter-as-gather (Jimenez 2014; BokehMe, Peng et al. 2022):
// every output texel reads every source texel within the slab's reach, and a
// source counts where its own disc covers the texel, with its energy spread
// over the disc's area (Dr.Bokeh's a(y)/(πr²)). So each light is one evenly
// filled disc at its own radius whatever the slab, its rim one texel wide,
// and there are no sparse taps. Bin b is gathered at the pyramid level where
// its radius is about 8 to 17 texels (4 to 9 for the first bin), so the work
// per texel is bounded; the rim is one texel of that level. Bins 0 to 2 are
// gathered from this pipeline's own pixels within the export halo; bins 3 to
// 5, wider than any halo, from the full-image map (`defocus_map_*`), built
// over the whole image as the Glow's is (pipeline/gpu/defocus.rs).
//
// Coverage. A slab's coverage at a texel is its scattered coverage divided by
// the share of the neighbourhood that nearer slabs leave visible (`v`), so
// what a nearer surface hides counts as continuing the slab's visible part:
// a blurred background is opaque right up to a sharp subject's edge, with no
// halo and none of the subject's colour, and fills in behind it. In the focus
// slab, a source behind a pixel's own sharp surface reaches it only within
// that surface's own radius (Dr.Bokeh's in-layer occlusion), so a sharp
// subject takes no colour from the slightly blurred scene behind it. Within a
// bin both surfaces are blurred alike, and as in the layered renderers their
// order is not kept: a hard cut there would follow the texel grid.
//
// Fill. A blurred foreground reveals what it hides out to its radius. Behind
// the focus range a wider slab always lies behind a narrower one and fills it
// statistically; in front, light left over after every slab takes the
// non-front image's mean at the foreground's scale, from a pyramid
// (`defocus_fill_*`).
//
// Boost. Before it is spread, a surface's energy is multiplied by up to the
// BOOST lane where it is bright (DEFOCUS_BOOST_STOPS above middle grey) and
// stands out from its surroundings (DEFOCUS_PROMINENCE_STOPS), which puts back
// what clipping took from lights; even areas such as the sky gain nothing.

// --- Constants ---

const DEFOCUS_PI: f32 = 3.14159265;
// The focus slab's largest radius ρ0, in shorter edges; bin b starts at ρ0·2^b.
const DEFOCUS_FOCUS_RADIUS: f32 = 0.001953125;
const DEFOCUS_BINS: u32 = 6u;
// Bins gathered from this pipeline's pixels; the others from the map.
const DEFOCUS_LOCAL_BINS: u32 = 3u;
// Octaves over which a bin's edge fades into the next.
const DEFOCUS_BIN_SOFTNESS: f32 = 0.1;
// A bin's widest radius, in its starting radius: 2^(1 + DEFOCUS_BIN_SOFTNESS).
const DEFOCUS_BIN_REACH: f32 = 2.144;
// Layers per slot: four per bin of sources, seven of slabs (two per bin and
// the fill).
const DEFOCUS_SOURCE_LAYERS: u32 = 4u;
const DEFOCUS_SLAB_LAYERS: u32 = 7u;
const DEFOCUS_FILL_LAYER: u32 = 6u;
// Cells of the full-image map along the longer edge (DEFOCUS_MAP_LONG in
// defocus.rs); bin 3 lies at this grid, each further bin at half of it.
const DEFOCUS_MAP_LONG: u32 = 768u;
// Edges of the occupancy blocks: 16 pixels for the focus slab, 8 texels for a
// bin at its level.
const DEFOCUS_FOCUS_BLOCK: i32 = 16;
const DEFOCUS_BIN_BLOCK: i32 = 8;
// Units of the focus slab's occupancy: radii in sixteenths of a pixel.
const DEFOCUS_OCCUPANCY_SCALE: f32 = 16.0;
// How much farther than a texel's own surface, in relative inverse
// distance, a source counts as behind it.
const DEFOCUS_DEPTH_TOLERANCE: f32 = 0.02;
// Shapes (BokehShape in defocus.rs).
const DEFOCUS_SHAPE_BUBBLE: f32 = 1.0;
const DEFOCUS_SHAPE_BLADES: f32 = 2.0;
const DEFOCUS_SHAPE_RING: f32 = 3.0;
const DEFOCUS_SHAPE_CATS_EYE: f32 = 4.0;
// Bubble: brightness 0.5 at the centre rising as r^6 to 2.0 at the rim; its
// integral over the unit disc.
const DEFOCUS_BUBBLE_CENTRE: f32 = 0.5;
const DEFOCUS_BUBBLE_RIM: f32 = 1.5;
const DEFOCUS_BUBBLE_AREA: f32 = 2.7489;
// Ring: the central obstruction's radius, in the disc's.
const DEFOCUS_RING_INNER: f32 = 0.55;
// Cat's eye: how far apart the two discs whose overlap it is lie at the
// image's corners, in disc radii; nothing at the centre.
const DEFOCUS_CATS_EYE_SHIFT: f32 = 0.9;
// Boost: brightness from 2 to 4.5 stops above middle grey, and standing out
// 1 to 3 stops above the surroundings, gain in full.
const DEFOCUS_MIDDLE_GREY: f32 = 0.18;
const DEFOCUS_BOOST_STOPS: vec2<f32> = vec2<f32>(2.0, 4.5);
const DEFOCUS_PROMINENCE_STOPS: vec2<f32> = vec2<f32>(1.0, 3.0);
// The surroundings' radius in a bin's starting radii, at most this many
// shorter edges so that it stays inside the export halo.
const DEFOCUS_SURROUND: f32 = 0.75;
const DEFOCUS_SURROUND_LIMIT: f32 = 0.0078125;
// A fill level counts once its non-front weight reaches this share.
const DEFOCUS_FILL_WEIGHT: f32 = 0.2;

// --- Bindings ---

// The composited Lens Blur, one layer per Lens Blur slot, which the first
// Blur stage blends by the mask (blur.wgsl).
@group(0) @binding(124) var defocus_result_tex: texture_2d_array<f32>;
// The focus slab's sources per pixel, two layers per slot (`prepare_defocus`).
@group(0) @binding(125) var defocus_focus_tex: texture_2d_array<f32>;
@group(0) @binding(126) var defocus_focus_out: texture_storage_2d_array<rgba16float, write>;
// Bin sources and slabs: the local ones over this pipeline at each bin's
// level (one mip per level from the first bin's), or the map's.
@group(0) @binding(127) var defocus_sources_tex: texture_2d_array<f32>;
@group(0) @binding(128) var defocus_sources_out: texture_storage_2d_array<rgba16float, write>;
@group(0) @binding(129) var defocus_slabs_tex: texture_2d_array<f32>;
@group(0) @binding(130) var defocus_slabs_out: texture_storage_2d_array<rgba16float, write>;
@group(0) @binding(131) var<uniform> defocus_pass: DefocusPass;
// Per slot, four regions of flags: the focus slab's largest radius in each
// block of 16 pixels, then whether each block of 8 texels of a local bin
// holds a source.
@group(0) @binding(132) var<storage, read_write> defocus_occupancy: array<atomic<u32>>;
// The map's slabs and its fill, read by the composite.
@group(0) @binding(133) var defocus_map_tex: texture_2d_array<f32>;
// The map's sums while tiles add to them (`defocus_cell_*`).
@group(0) @binding(134) var<storage, read_write> defocus_cells: array<vec4<f32>>;
@group(0) @binding(135) var defocus_result_out: texture_storage_2d_array<rgba16float, write>;

struct DefocusPass {
    // The bin a pass works on and the mip it writes.
    bin: u32,
    mip: u32,
    // 1 when the full-image map is built.
    map_ready: u32,
    unused: u32,
    // The pipeline level of each local bin (xyz).
    levels: vec4<u32>,
}

// --- Slots ---

fn defocus_layer_count(capacity: u32) -> u32 {
    var layers = 0u;
    for (var index = 0u; index < Common::mask_slot_count(); index = index + 1u) {
        if Common::mask_slot_applies(Common::mask_data[index].metadata, MASK_EFFECT_DEFOCUS_ID) {
            layers = layers + 1u;
        }
    }
    return min(layers, capacity);
}

// The slot whose layers are `layer`: the layer-th Lens Blur slot.
fn defocus_slot(layer: u32) -> u32 {
    var seen = 0u;
    for (var index = 0u; index < Common::mask_slot_count(); index = index + 1u) {
        if !Common::mask_slot_applies(Common::mask_data[index].metadata, MASK_EFFECT_DEFOCUS_ID) {
            continue;
        }
        if seen == layer { return index; }
        seen = seen + 1u;
    }
    return 0u;
}

// The layer of Lens Blur slot `index`.
fn defocus_layer_of(index: u32) -> u32 {
    var layer = 0u;
    for (var other = 0u; other < index; other = other + 1u) {
        if Common::mask_slot_applies(Common::mask_data[other].metadata, MASK_EFFECT_DEFOCUS_ID) {
            layer = layer + 1u;
        }
    }
    return layer;
}

// The blurred image of Lens Blur slot `index` at tile pixel `pos`, for the
// first Blur stage.
fn mask_defocus_at(pos: vec2<i32>, index: u32) -> vec3<f32> {
    return textureLoad(defocus_result_tex, pos, defocus_layer_of(index), 0).rgb;
}

// --- Scale ---

// Pipeline pixels per shorter edge.
fn defocus_short_pixels() -> f32 {
    return effect_short_edge();
}

// A bin's starting radius ρ0·2^b, in shorter edges.
fn defocus_bin_start(bin: u32) -> f32 {
    return DEFOCUS_FOCUS_RADIUS * f32(1u << bin);
}

// The pipeline level of local bin `bin`.
fn defocus_local_level(bin: u32) -> u32 {
    switch bin {
        case 0u: { return defocus_pass.levels.x; }
        case 1u: { return defocus_pass.levels.y; }
        default: { return defocus_pass.levels.z; }
    }
}

// The first texel of pipeline level `level` in this tile, on the full image's
// grid of that level.
fn defocus_level_origin(level: u32) -> vec2<i32> {
    return vec2<i32>(floor(vec2<f32>(Common::tile_origin()) / f32(1u << level)));
}

// The map's slab mip of bin `bin` (3 to 5): bins 4 and 5 are written at
// twice their sources' resolution (DEFOCUS_MAP_SLAB_MIPS in defocus.rs).
fn defocus_map_slab_mip(bin: u32) -> u32 {
    return max(bin, DEFOCUS_LOCAL_BINS + 1u) - DEFOCUS_LOCAL_BINS - 1u;
}

// The map's grid of bin `bin` (3 to 5), as Common::full_image_grid halves it.
fn defocus_map_grid(bin: u32) -> vec2<u32> {
    return max(Common::full_image_grid(DEFOCUS_MAP_LONG) >> vec2<u32>(bin - DEFOCUS_LOCAL_BINS), vec2<u32>(1u));
}

// --- Controls ---

struct DefocusAperture {
    shape: f32,
    blades: f32,
    rotation: f32,
    // The polygon's inradius and the angle each blade spans.
    apothem: f32,
    sector: f32,
}

fn defocus_aperture(params: MaskEffectParams) -> DefocusAperture {
    let blades = clamp(round(mask_effect_lane(params, DEFOCUS_BLADES_LANE)), 3.0, 12.0);
    return DefocusAperture(
        mask_effect_lane(params, DEFOCUS_SHAPE_LANE),
        blades,
        mask_effect_lane(params, DEFOCUS_ROTATION_LANE),
        cos(DEFOCUS_PI / blades),
        2.0 * DEFOCUS_PI / blades,
    );
}

// The largest radius, in shorter edges.
fn defocus_largest_radius(params: MaskEffectParams) -> f32 {
    let full = max(
        vec2<f32>(f32(Common::camera_uniforms.full_width), f32(Common::camera_uniforms.full_height)),
        vec2<f32>(1.0),
    );
    let aspect = max(full.x, full.y) / min(full.x, full.y);
    // Bounded by the last bin's reach.
    return min(
        mask_effect_lane(params, DEFOCUS_RADIUS_LANE) * aspect,
        defocus_bin_start(DEFOCUS_BINS - 1u) * 2.0,
    );
}

fn defocus_by_depth(params: MaskEffectParams) -> bool {
    return params.options[DEFOCUS_DEPTH_OPTION] > 0.5 && Geometry::scene_geometry_present();
}

// --- Circles of confusion ---

// Q = D_far / D_near, the scene's depth ratio, at least 1.
fn defocus_depth_ratio() -> f32 {
    let camera = Bake::scene_bake.camera;
    return max(camera.anchors.w / camera.anchors.z, 1.0);
}

// A surface's inverse distance relative to the nearest surface's, D_near / D:
// 1 for the nearest, 1 / Q for the farthest.
fn defocus_inverse(surface: Geometry::SceneSurface) -> f32 {
    return 1.0 / (1.0 + (defocus_depth_ratio() - 1.0) * surface.d);
}

// The focus range in inverse distance, as a control depth c gives it (Q^-c):
// its near end (x), its far end (y), and the greatest distance in it of any
// surface from the range (z). A flat scene has none: z is 0.
fn defocus_focus(params: MaskEffectParams) -> vec3<f32> {
    let ratio = defocus_depth_ratio();
    let near = pow(ratio, -mask_effect_lane(params, DEFOCUS_FOCUS_NEAR_LANE));
    let far = pow(ratio, -mask_effect_lane(params, DEFOCUS_FOCUS_FAR_LANE));
    return vec3<f32>(near, far, max(1.0 - near, far - 1.0 / ratio));
}

// A surface's signed circle of confusion in shorter edges, negative in front
// of the focus range. The thin lens's is linear in inverse distance: 0 in the
// range and `radius` at the scene's most out-of-focus end. The photo's own
// defocus there (the B7 fit, bake_focus.wgsl, a deviation in shorter edges of
// 1 / z of the frame) is taken away in quadrature, a disc of radius r having
// deviation r / 2, so a surface the lens already softened is topped up to
// the total rather than blurred twice. The fit's flat term is left out: it is
// the working image's resampling, not the lens.
fn defocus_coc(surface: Geometry::SceneSurface, focus: vec3<f32>, radius: f32) -> f32 {
    if focus.z <= 1e-6 {
        return 0.0;
    }
    let inverse = defocus_inverse(surface);
    let front = max(inverse - focus.x, 0.0);
    let back = max(focus.y - inverse, 0.0);
    let total = radius * min((front + back) / focus.z, 1.0);
    let fit = Bake::scene_bake.focus_fit;
    let lens = select(0.0, fit.x * abs(1.0 / surface.position.z - fit.y), fit.w != 0.0);
    let added = sqrt(max(total * total - 4.0 * lens * lens, 0.0));
    return select(added, -added, front > 0.0);
}

struct DefocusSurface {
    // Colour times coverage.
    premultiplied: vec3<f32>,
    alpha: f32,
    // Signed circle of confusion, in shorter edges.
    coc: f32,
    // Inverse distance D_near / D (1 without depth).
    depth: f32,
}

struct DefocusPixel {
    near: DefocusSurface,
    far: DefocusSurface,
}

// The surfaces of tile pixel `pos` for Lens Blur slot `index`: the pixel's
// own colour split between its two surfaces by their coverage
// (Geometry::scene_split), or one surface without depth. Unmasked pixels stay
// in focus.
fn defocus_pixel(pos: vec2<i32>, index: u32, params: MaskEffectParams) -> DefocusPixel {
    let p = Common::clamp_pos(pos);
    let center = SceneAdjustments::local_effects_at(p);
    let in_mask = select(0.0, 1.0, SceneAdjustments::local_mask_weight(p, index) > 0.0);
    let radius = defocus_largest_radius(params) * in_mask;
    if !defocus_by_depth(params) {
        return DefocusPixel(
            DefocusSurface(center, 1.0, radius, 1.0),
            DefocusSurface(vec3<f32>(0.0), 0.0, radius, 1.0),
        );
    }
    let scene_rgb = Geometry::scene_unedited_at(p);
    let pixel = Geometry::scene_pixel(full_image_uv(p), scene_rgb, 1.0);
    let colors = Geometry::scene_split(pixel, scene_rgb, center);
    let near_alpha = select(1.0, pixel.coverage, pixel.in_band);
    let focus = defocus_focus(params);
    return DefocusPixel(
        DefocusSurface(
            colors[0] * near_alpha,
            near_alpha,
            defocus_coc(pixel.near, focus, radius),
            defocus_inverse(pixel.near),
        ),
        DefocusSurface(
            colors[1] * (1.0 - near_alpha),
            1.0 - near_alpha,
            defocus_coc(pixel.far, focus, radius),
            defocus_inverse(pixel.far),
        ),
    );
}

// --- Slab membership ---

// |c| in octaves above ρ0.
fn defocus_octave(coc: f32) -> f32 {
    return log2(max(abs(coc), 1e-9) / DEFOCUS_FOCUS_RADIUS);
}

// 0 below octave `edge`, 1 above, fading over DEFOCUS_BIN_SOFTNESS on
// either side.
fn defocus_above(octave: f32, edge: f32) -> f32 {
    return clamp((octave - edge + DEFOCUS_BIN_SOFTNESS) / (2.0 * DEFOCUS_BIN_SOFTNESS), 0.0, 1.0);
}

// The share of a surface of circle `coc` in bin `bin` (on its side).
fn defocus_bin_weight(coc: f32, bin: u32) -> f32 {
    let octave = defocus_octave(coc);
    let above = defocus_above(octave, f32(bin));
    if bin + 1u >= DEFOCUS_BINS {
        return above;
    }
    return above - defocus_above(octave, f32(bin + 1u));
}

// The share of it in the focus slab.
fn defocus_focus_weight(coc: f32) -> f32 {
    return 1.0 - defocus_above(defocus_octave(coc), 0.0);
}

// The share of a surface of circle `coc` that lies no nearer than bin `bin`
// on `side` (0 front, 1 back): what the bin may show where it is not covered.
fn defocus_visible(coc: f32, bin: u32, side: u32) -> f32 {
    let octave = defocus_octave(coc);
    if side == 1u {
        // Behind the range: back bins from `bin` on.
        return select(0.0, defocus_above(octave, f32(bin)), coc > 0.0);
    }
    // In front: all but the front bins past `bin`.
    let nearer = select(0.0, defocus_above(octave, f32(bin + 1u)), coc < 0.0);
    return 1.0 - nearer;
}

// The share of a surface of circle `coc` that lies no nearer than the focus
// slab: all but its front bins' shares.
fn defocus_focus_visible(coc: f32) -> f32 {
    return 1.0 - select(0.0, defocus_above(defocus_octave(coc), 0.0), coc < 0.0);
}

// --- The aperture ---

// How much of the aperture of a source covers offset `u` from it, in its
// radius, anti-aliased over `edge` radii (one texel), times its brightness
// there for Bubble. `point` is the source's position from the image centre in
// half-diagonals, which the Cat's eye follows.
fn defocus_kernel(aperture: DefocusAperture, u: vec2<f32>, edge: f32, point: vec2<f32>) -> f32 {
    let r = length(u);
    let half = 0.5 * edge;
    if aperture.shape == DEFOCUS_SHAPE_BLADES {
        let theta = atan2(u.y, u.x) - aperture.rotation;
        let within = (fract(theta / aperture.sector + 0.5) - 0.5) * aperture.sector;
        let boundary = aperture.apothem / max(cos(within), 0.25);
        return smoothstep(boundary + half, boundary - half, r);
    }
    let disc = smoothstep(1.0 + half, 1.0 - half, r);
    if aperture.shape == DEFOCUS_SHAPE_BUBBLE {
        let r2 = min(r * r, 1.0);
        return disc * (DEFOCUS_BUBBLE_CENTRE + DEFOCUS_BUBBLE_RIM * r2 * r2 * r2);
    }
    if aperture.shape == DEFOCUS_SHAPE_RING {
        return disc - smoothstep(DEFOCUS_RING_INNER + half, DEFOCUS_RING_INNER - half, r);
    }
    if aperture.shape == DEFOCUS_SHAPE_CATS_EYE {
        let shift = 0.5 * DEFOCUS_CATS_EYE_SHIFT * point;
        let a = smoothstep(1.0 + half, 1.0 - half, length(u - shift));
        let b = smoothstep(1.0 + half, 1.0 - half, length(u + shift));
        return min(a, b);
    }
    return disc;
}

// The aperture's area in squared radii, for a source at `point`.
fn defocus_kernel_area(aperture: DefocusAperture, point: vec2<f32>) -> f32 {
    if aperture.shape == DEFOCUS_SHAPE_BLADES {
        return 0.5 * aperture.blades * sin(aperture.sector);
    }
    if aperture.shape == DEFOCUS_SHAPE_BUBBLE {
        return DEFOCUS_BUBBLE_AREA;
    }
    if aperture.shape == DEFOCUS_SHAPE_RING {
        return DEFOCUS_PI * (1.0 - DEFOCUS_RING_INNER * DEFOCUS_RING_INNER);
    }
    if aperture.shape == DEFOCUS_SHAPE_CATS_EYE {
        // The lens two unit discs form at distance d.
        let d = min(DEFOCUS_CATS_EYE_SHIFT * length(point), 1.9);
        return 2.0 * acos(0.5 * d) - 0.5 * d * sqrt(4.0 - d * d);
    }
    return DEFOCUS_PI;
}

// Radii below which a source's energy is normalized by its kernel's sum over
// the texels it covers rather than by its area: the two differ there by up
// to a fifth, by where the source lies between texels, which would show as
// a pattern.
const DEFOCUS_LATTICE_RADIUS: f32 = 3.0;

// The weight a source spreads over the texel at `offset` texels from it, of
// radius `radius` texels: its aperture there over its total, so its energy
// sums to one over the texels it covers.
fn defocus_spread(aperture: DefocusAperture, offset: vec2<f32>, radius: f32, point: vec2<f32>) -> f32 {
    let weight = defocus_kernel(aperture, offset / radius, 1.0 / radius, point);
    if weight <= 0.0 {
        return 0.0;
    }
    if radius >= DEFOCUS_LATTICE_RADIUS {
        return weight / (defocus_kernel_area(aperture, point) * radius * radius);
    }
    // The texels around the source lie at its phase plus whole texels.
    let phase = offset - round(offset);
    let reach = i32(ceil(radius + 1.0));
    var total = 0.0;
    for (var y = -reach; y <= reach; y = y + 1) {
        for (var x = -reach; x <= reach; x = x + 1) {
            total += defocus_kernel(aperture, (phase + vec2<f32>(f32(x), f32(y))) / radius, 1.0 / radius, point);
        }
    }
    return weight / max(total, 1e-6);
}

// Full-image uv from the image centre in half-diagonals.
fn defocus_point(uv: vec2<f32>) -> vec2<f32> {
    let full = max(
        vec2<f32>(f32(Common::camera_uniforms.full_width), f32(Common::camera_uniforms.full_height)),
        vec2<f32>(1.0),
    );
    return (uv - vec2<f32>(0.5)) * full / (0.5 * length(full));
}

// --- Boost ---

// The energy gain of a surface of luminance `luma` among surroundings of mean
// luminance `surround`.
fn defocus_boost(luma: f32, surround: f32, gain: f32) -> f32 {
    if gain <= 1.0 || luma <= 0.0 {
        return 1.0;
    }
    let bright = smoothstep(DEFOCUS_BOOST_STOPS.x, DEFOCUS_BOOST_STOPS.y, log2(luma / DEFOCUS_MIDDLE_GREY));
    let prominent = smoothstep(
        DEFOCUS_PROMINENCE_STOPS.x,
        DEFOCUS_PROMINENCE_STOPS.y,
        log2(luma / max(surround, 1e-5)),
    );
    return 1.0 + (gain - 1.0) * bright * prominent;
}

// The mean luminance around tile pixel `centre` within `radius` pixels: a
// 4 × 4 grid over the square.
fn defocus_surround(centre: vec2<f32>, radius: f32) -> f32 {
    var sum = 0.0;
    for (var y = 0; y < 4; y = y + 1) {
        for (var x = 0; x < 4; x = x + 1) {
            let offset = (vec2<f32>(f32(x), f32(y)) - vec2<f32>(1.5)) * (radius / 1.5);
            let tap = Common::clamp_pos(vec2<i32>(floor(centre + offset)));
            sum += Common::safe_luma(max(SceneAdjustments::local_effects_at(tap), vec3<f32>(0.0)));
        }
    }
    return sum / 16.0;
}

// The surroundings' radius of bin `bin`, in pipeline pixels.
fn defocus_surround_radius(bin: u32) -> f32 {
    return min(DEFOCUS_SURROUND * defocus_bin_start(bin), DEFOCUS_SURROUND_LIMIT) * defocus_short_pixels();
}

// --- Block sums ---

// What a bin's sources sum over a block of pixels: per side the boosted
// premultiplied colour and coverage, the coverage-weighted |c| and position
// in the block, and what the bin may show (`defocus_visible`). The position
// places each source's disc at its pixels' centroid rather than the texel's
// centre, so the rims of lines and edges stay smooth at any level.
struct DefocusBinSums {
    front: vec4<f32>,
    back: vec4<f32>,
    // |c| · α front and back, visible share front and back.
    extra: vec4<f32>,
    // Position · α front (xy) and back (zw), in texels from the block's
    // centre.
    centroid: vec4<f32>,
}

fn defocus_add_surface(
    sums: ptr<function, DefocusBinSums>,
    surface: DefocusSurface,
    bin: u32,
    surround: f32,
    gain: f32,
    position: vec2<f32>,
) {
    (*sums).extra.z += surface.alpha * defocus_visible(surface.coc, bin, 0u);
    (*sums).extra.w += surface.alpha * defocus_visible(surface.coc, bin, 1u);
    let share = surface.alpha * defocus_bin_weight(surface.coc, bin);
    if share <= 0.0 || surface.coc == 0.0 {
        return;
    }
    let color = surface.premultiplied / max(surface.alpha, 1e-6);
    let boost = defocus_boost(Common::safe_luma(max(color, vec3<f32>(0.0))), surround, gain);
    let energy = vec4<f32>(color * (share * boost), share);
    if surface.coc < 0.0 {
        (*sums).front += energy;
        (*sums).extra.x += share * abs(surface.coc);
        (*sums).centroid.x += share * position.x;
        (*sums).centroid.y += share * position.y;
    } else {
        (*sums).back += energy;
        (*sums).extra.y += share * abs(surface.coc);
        (*sums).centroid.z += share * position.x;
        (*sums).centroid.w += share * position.y;
    }
}

// Adds tile pixel `pos`, which lies at `position` texels from the block's
// centre.
fn defocus_add_pixel(
    sums: ptr<function, DefocusBinSums>,
    pos: vec2<i32>,
    position: vec2<f32>,
    index: u32,
    params: MaskEffectParams,
    bin: u32,
    surround: f32,
) {
    let pixel = defocus_pixel(pos, index, params);
    let gain = mask_effect_lane(params, DEFOCUS_BOOST_LANE);
    defocus_add_surface(sums, pixel.near, bin, surround, gain, position);
    if pixel.far.alpha > 0.0 {
        defocus_add_surface(sums, pixel.far, bin, surround, gain, position);
    }
}

// The four source layers of a bin from its sums over `count` pixels: means,
// with |c| and the centroid per unit of coverage.
fn defocus_store_sources(
    texel: vec2<i32>,
    layer: u32,
    sums: DefocusBinSums,
    count: f32,
) {
    let scale = 1.0 / max(count, 1.0);
    let coc = vec2<f32>(
        sums.extra.x / max(sums.front.w, 1e-9),
        sums.extra.y / max(sums.back.w, 1e-9),
    );
    let centroid = vec4<f32>(
        sums.centroid.xy / max(sums.front.w, 1e-9),
        sums.centroid.zw / max(sums.back.w, 1e-9),
    );
    textureStore(defocus_sources_out, texel, layer, sums.front * scale);
    textureStore(defocus_sources_out, texel, layer + 1u, sums.back * scale);
    textureStore(defocus_sources_out, texel, layer + 2u, vec4<f32>(coc, sums.extra.zw * scale));
    textureStore(defocus_sources_out, texel, layer + 3u, centroid);
}

// --- Occupancy ---

// Blocks per row of a region, enough for every level.
fn defocus_occupancy_stride() -> u32 {
    return (Common::camera_uniforms.width + 7u) / 8u + 1u;
}

fn defocus_occupancy_region(layer: u32, region: u32) -> u32 {
    let rows = (Common::camera_uniforms.height + 7u) / 8u + 1u;
    return (layer * 4u + region) * defocus_occupancy_stride() * rows;
}

// Whether `block` lies in a region's grid.
fn defocus_occupancy_valid(block: vec2<i32>) -> bool {
    let rows = i32((Common::camera_uniforms.height + 7u) / 8u + 1u);
    return all(block >= vec2<i32>(0)) && block.x < i32(defocus_occupancy_stride()) && block.y < rows;
}

fn defocus_occupancy_index(layer: u32, region: u32, block: vec2<i32>) -> u32 {
    return defocus_occupancy_region(layer, region) + u32(block.y) * defocus_occupancy_stride() + u32(block.x);
}

// --- Focus slab sources ---

// Per pixel, the focus slab's share of its surfaces merged: premultiplied
// colour and coverage in the slot's first layer; |c| in pipeline pixels, what
// the focus slab may show and depth in its second.
@compute @workgroup_size(8, 8, 1)
fn prepare_defocus(@builtin(global_invocation_id) gid: vec3<u32>) {
    let size = textureDimensions(defocus_focus_out);
    let layers = defocus_layer_count(textureNumLayers(defocus_focus_out) / 2u);
    if any(gid.xy >= size) || gid.z >= layers { return; }
    let pos = vec2<i32>(gid.xy);
    let index = defocus_slot(gid.z);
    let params = mask_effect_params(index);
    let pixel = defocus_pixel(pos, index, params);
    var premultiplied = vec3<f32>(0.0);
    var alpha = 0.0;
    var coc = 0.0;
    var depth = 0.0;
    var visible = 0.0;
    for (var side = 0u; side < 2u; side = side + 1u) {
        var surface = pixel.near;
        if side == 1u { surface = pixel.far; }
        let share = surface.alpha * defocus_focus_weight(surface.coc);
        premultiplied += surface.premultiplied * defocus_focus_weight(surface.coc);
        alpha += share;
        coc += share * abs(surface.coc);
        depth += share * surface.depth;
        visible += surface.alpha * defocus_focus_visible(surface.coc);
    }
    let radius = coc / max(alpha, 1e-9) * defocus_short_pixels();
    let mean_depth = select(1.0, depth / max(alpha, 1e-9), alpha > 0.0);
    textureStore(defocus_focus_out, pos, 2u * gid.z, vec4<f32>(premultiplied, alpha));
    textureStore(defocus_focus_out, pos, 2u * gid.z + 1u, vec4<f32>(radius, visible, mean_depth, 0.0));
    if alpha > 0.0 && radius >= 0.5 {
        let block = pos / DEFOCUS_FOCUS_BLOCK;
        atomicMax(
            &defocus_occupancy[defocus_occupancy_index(gid.z, 0u, block)],
            u32(ceil(radius * DEFOCUS_OCCUPANCY_SCALE)),
        );
    }
}

struct DefocusFocusSource {
    premultiplied: vec3<f32>,
    alpha: f32,
    // In pipeline pixels.
    radius: f32,
    visible: f32,
    depth: f32,
}

fn defocus_focus_source(pos: vec2<i32>, layer: u32) -> DefocusFocusSource {
    let p = Common::clamp_pos(pos);
    let colour = textureLoad(defocus_focus_tex, p, 2u * layer, 0);
    let extra = textureLoad(defocus_focus_tex, p, 2u * layer + 1u, 0);
    return DefocusFocusSource(colour.rgb, colour.w, extra.x, extra.y, extra.z);
}

// --- Bin sources ---

// Sources of local bin `defocus_pass.bin` at its level: per texel, the mean
// of the 2^level × 2^level pixels it covers on the full image's grid.
@compute @workgroup_size(8, 8, 1)
fn build_defocus_sources(@builtin(global_invocation_id) gid: vec3<u32>) {
    let size = textureDimensions(defocus_sources_out);
    let capacity = textureNumLayers(defocus_sources_out) / (DEFOCUS_LOCAL_BINS * DEFOCUS_SOURCE_LAYERS);
    let layers = defocus_layer_count(capacity);
    if any(gid.xy >= size) || gid.z >= layers { return; }
    let bin = defocus_pass.bin;
    let level = defocus_local_level(bin);
    let step = 1 << level;
    let global = vec2<i32>(gid.xy) + defocus_level_origin(level);
    let first = global * step - Common::tile_origin();
    let index = defocus_slot(gid.z);
    let params = mask_effect_params(index);
    let surround = defocus_surround(vec2<f32>(first) + vec2<f32>(0.5 * f32(step)), defocus_surround_radius(bin));
    var sums = DefocusBinSums(vec4<f32>(0.0), vec4<f32>(0.0), vec4<f32>(0.0), vec4<f32>(0.0));
    let centre = 0.5 * f32(step - 1);
    for (var y = 0; y < step; y = y + 1) {
        for (var x = 0; x < step; x = x + 1) {
            let position = (vec2<f32>(f32(x), f32(y)) - vec2<f32>(centre)) / f32(step);
            defocus_add_pixel(&sums, first + vec2<i32>(x, y), position, index, params, bin, surround);
        }
    }
    let layer = (gid.z * DEFOCUS_LOCAL_BINS + bin) * DEFOCUS_SOURCE_LAYERS;
    defocus_store_sources(vec2<i32>(gid.xy), layer, sums, f32(step * step));
    if sums.front.w + sums.back.w > 0.0 {
        let block = vec2<i32>(gid.xy) / DEFOCUS_BIN_BLOCK;
        atomicOr(&defocus_occupancy[defocus_occupancy_index(gid.z, 1u + bin, block)], 1u);
    }
}

// --- Gather ---

var<workgroup> defocus_group_occupied: atomic<u32>;

// The slabs of bin `defocus_pass.bin` from its sources: per texel and side,
// the scatter of every source within the bin's reach, normalized by what
// nearer slabs leave visible. Local bins read and write this pipeline's
// textures at the bin's level. Map bins (3 to 5) read the map's sources at
// mip `bin - 3` and write its slabs at mip `defocus_pass.mip`, which may be
// finer than their sources, so their one-texel rims are drawn smoothly
// rather than by the composite's bilinear upsampling.
@compute @workgroup_size(8, 8, 1)
fn gather_defocus(
    @builtin(global_invocation_id) gid: vec3<u32>,
    @builtin(local_invocation_index) local_index: u32,
    @builtin(workgroup_id) group: vec3<u32>,
) {
    let bin = defocus_pass.bin;
    let local = bin < DEFOCUS_LOCAL_BINS;
    let size = vec2<i32>(textureDimensions(defocus_slabs_out));
    let capacity = textureNumLayers(defocus_slabs_out) / DEFOCUS_SLAB_LAYERS;
    let layers = defocus_layer_count(capacity);
    let inside = all(vec2<i32>(gid.xy) < size) && gid.z < layers;
    let slot_layer = min(gid.z, max(layers, 1u) - 1u);
    // The bin's source texel in shorter edges, its source mip, the output's
    // texels per source texel and the bin's reach in source texels.
    var texel = 0.0;
    var mip = 0u;
    var source_bin = 0u;
    var fineness = 1.0;
    if local {
        let level = defocus_local_level(bin);
        texel = f32(1u << level) / defocus_short_pixels();
        mip = level - defocus_pass.levels.x;
        source_bin = bin;
    } else {
        let grid = defocus_map_grid(bin);
        texel = 1.0 / f32(min(grid.x, grid.y));
        mip = bin - DEFOCUS_LOCAL_BINS;
        source_bin = bin - DEFOCUS_LOCAL_BINS;
        fineness = f32(1u << (mip - defocus_pass.mip));
    }
    let start = defocus_bin_start(bin) / texel;
    let reach = i32(ceil(DEFOCUS_BIN_REACH * start + 1.0));

    // Skip groups with no source within reach (local bins only, whose output
    // is their sources' grid).
    if local {
        if local_index == 0u {
            atomicStore(&defocus_group_occupied, 0u);
        }
        workgroupBarrier();
        let span = reach / DEFOCUS_BIN_BLOCK + 1;
        let side = 2 * span + 1;
        let own = vec2<i32>(group.xy);
        for (var k = i32(local_index); k < side * side; k = k + 64) {
            let block = own + vec2<i32>(k % side, k / side) - vec2<i32>(span);
            if defocus_occupancy_valid(block) {
                let flag = atomicLoad(&defocus_occupancy[defocus_occupancy_index(slot_layer, 1u + bin, block)]);
                if flag != 0u {
                    atomicOr(&defocus_group_occupied, 1u);
                }
            }
        }
        workgroupBarrier();
        if atomicLoad(&defocus_group_occupied) == 0u {
            if inside {
                let out_layer = gid.z * DEFOCUS_SLAB_LAYERS + 2u * source_bin;
                textureStore(defocus_slabs_out, vec2<i32>(gid.xy), out_layer, vec4<f32>(0.0));
                textureStore(defocus_slabs_out, vec2<i32>(gid.xy), out_layer + 1u, vec4<f32>(0.0));
            }
            return;
        }
    }
    if !inside { return; }

    let index = defocus_slot(gid.z);
    let params = mask_effect_params(index);
    let aperture = defocus_aperture(params);
    let source_layer = (gid.z * DEFOCUS_LOCAL_BINS + source_bin) * DEFOCUS_SOURCE_LAYERS;
    let source_size = vec2<i32>(textureDimensions(defocus_sources_tex, mip));
    // The output texel's centre in source texels (centres at integers).
    let here = (vec2<f32>(gid.xy) + vec2<f32>(0.5)) / fineness - vec2<f32>(0.5);
    let nearest = vec2<i32>(floor(here + vec2<f32>(0.5)));
    // A source texel's position over the full image, for the Cat's eye.
    var origin = vec2<f32>(0.5);
    var uv_scale = vec2<f32>(0.0);
    let full = max(
        vec2<f32>(f32(Common::camera_uniforms.full_width), f32(Common::camera_uniforms.full_height)),
        vec2<f32>(1.0),
    );
    if local {
        let level = defocus_local_level(bin);
        origin = vec2<f32>(defocus_level_origin(level)) + vec2<f32>(0.5);
        uv_scale = vec2<f32>(f32(1u << level)) / full;
    } else {
        uv_scale = 1.0 / vec2<f32>(defocus_map_grid(bin));
    }
    let nominal = 1.5 * start;
    var colour = array<vec3<f32>, 2>(vec3<f32>(0.0), vec3<f32>(0.0));
    var scattered = vec2<f32>(0.0);
    var visible = vec2<f32>(0.0);
    var area = 0.0;
    for (var dy = -reach; dy <= reach; dy = dy + 1) {
        for (var dx = -reach; dx <= reach; dx = dx + 1) {
            let tap = nearest + vec2<i32>(dx, dy);
            let grid_offset = here - vec2<f32>(tap);
            if length(grid_offset) > f32(reach) { continue; }
            let clamped = clamp(tap, vec2<i32>(0), source_size - vec2<i32>(1));
            let nominal_weight = smoothstep(nominal + 0.5, nominal - 0.5, length(grid_offset));
            area += nominal_weight;
            let extra = textureLoad(defocus_sources_tex, clamped, source_layer + 2u, mip);
            visible += nominal_weight * extra.zw;
            let front = textureLoad(defocus_sources_tex, clamped, source_layer, mip);
            let back = textureLoad(defocus_sources_tex, clamped, source_layer + 1u, mip);
            if front.w + back.w <= 0.0 { continue; }
            let centroid = textureLoad(defocus_sources_tex, clamped, source_layer + 3u, mip);
            let point = defocus_point((vec2<f32>(tap) + origin) * uv_scale);
            for (var side = 0u; side < 2u; side = side + 1u) {
                let source = select(front, back, side == 1u);
                if source.w <= 0.0 { continue; }
                let radius = max(extra[side] / texel, 0.5);
                // From the source's centroid to the output texel.
                let offset = grid_offset - select(centroid.xy, centroid.zw, side == 1u);
                // The rim is one source texel wide: narrower, the rims of
                // neighbouring sources of an extended light would show as
                // rings at the sources' spacing.
                let weight = defocus_spread(aperture, offset, radius, point);
                if weight <= 0.0 { continue; }
                colour[side] += weight * source.rgb;
                scattered[side] += weight * source.w;
            }
        }
    }
    let out_layer = gid.z * DEFOCUS_SLAB_LAYERS + 2u * source_bin;
    for (var side = 0u; side < 2u; side = side + 1u) {
        let shown = visible[side] / max(area, 1e-6);
        let coverage = clamp(scattered[side] / max(shown, 0.05), 0.0, 1.0);
        let premultiplied = colour[side] * (coverage / max(scattered[side], 1e-9));
        textureStore(
            defocus_slabs_out,
            vec2<i32>(gid.xy),
            out_layer + side,
            select(vec4<f32>(0.0), vec4<f32>(premultiplied, coverage), scattered[side] > 0.0),
        );
    }
}

// --- Fill ---

// The fill's first level, at the first local bin's level: per texel the mean
// premultiplied colour of what lies behind the front slabs, and its share.
@compute @workgroup_size(8, 8, 1)
fn build_defocus_fill(@builtin(global_invocation_id) gid: vec3<u32>) {
    let size = textureDimensions(defocus_slabs_out);
    let capacity = textureNumLayers(defocus_slabs_out) / DEFOCUS_SLAB_LAYERS;
    let layers = defocus_layer_count(capacity);
    if any(gid.xy >= size) || gid.z >= layers { return; }
    let level = defocus_pass.levels.x;
    let step = 1 << level;
    let global = vec2<i32>(gid.xy) + defocus_level_origin(level);
    let first = global * step - Common::tile_origin();
    let index = defocus_slot(gid.z);
    let params = mask_effect_params(index);
    var sum = vec4<f32>(0.0);
    for (var y = 0; y < step; y = y + 1) {
        for (var x = 0; x < step; x = x + 1) {
            sum += defocus_fill_pixel(first + vec2<i32>(x, y), index, params);
        }
    }
    textureStore(
        defocus_slabs_out,
        vec2<i32>(gid.xy),
        gid.z * DEFOCUS_SLAB_LAYERS + DEFOCUS_FILL_LAYER,
        sum / f32(step * step),
    );
}

// What a pixel adds to the fill: the premultiplied colour and coverage of
// its surfaces that lie behind the front slabs.
fn defocus_fill_pixel(pos: vec2<i32>, index: u32, params: MaskEffectParams) -> vec4<f32> {
    let pixel = defocus_pixel(pos, index, params);
    var sum = vec4<f32>(0.0);
    for (var side = 0u; side < 2u; side = side + 1u) {
        var surface = pixel.near;
        if side == 1u { surface = pixel.far; }
        sum += vec4<f32>(surface.premultiplied, surface.alpha) * defocus_focus_visible(surface.coc);
    }
    return sum;
}

// Fill mip `defocus_pass.mip` from the one before: the mean of the parent
// texels its texel covers on the full image's grid. Local fills sit at
// `level_0 + mip`; the map's at the map grid's mips.
@compute @workgroup_size(8, 8, 1)
fn halve_defocus_fill(@builtin(global_invocation_id) gid: vec3<u32>) {
    let size = textureDimensions(defocus_slabs_out);
    let capacity = textureNumLayers(defocus_slabs_out) / DEFOCUS_SLAB_LAYERS;
    let layers = defocus_layer_count(capacity);
    if any(gid.xy >= size) || gid.z >= layers { return; }
    let parent_size = vec2<i32>(textureDimensions(defocus_slabs_tex));
    var first = 2 * vec2<i32>(gid.xy);
    if defocus_pass.bin < DEFOCUS_LOCAL_BINS {
        // Local: global texel g covers parent globals 2g and 2g + 1.
        let level = defocus_pass.levels.x + defocus_pass.mip;
        let global = vec2<i32>(gid.xy) + defocus_level_origin(level);
        first = 2 * global - defocus_level_origin(level - 1u);
    }
    let layer = gid.z * DEFOCUS_SLAB_LAYERS + DEFOCUS_FILL_LAYER;
    var sum = vec4<f32>(0.0);
    for (var y = 0; y < 2; y = y + 1) {
        for (var x = 0; x < 2; x = x + 1) {
            let tap = clamp(first + vec2<i32>(x, y), vec2<i32>(0), parent_size - vec2<i32>(1));
            sum += textureLoad(defocus_slabs_tex, tap, layer, 0);
        }
    }
    textureStore(defocus_slabs_out, vec2<i32>(gid.xy), layer, 0.25 * sum);
}

// --- Full-image map ---

// Vectors per cell of the map's sums: front, back, extra, centroid.
const DEFOCUS_CELL_VECTORS: u32 = 4u;

// The map's cells per slot: bins 3 to 5 at their grids, then the fill at
// bin 3's grid.
fn defocus_cell_offset(bin: u32) -> u32 {
    var offset = 0u;
    for (var b = DEFOCUS_LOCAL_BINS; b < bin; b = b + 1u) {
        let grid = defocus_map_grid(b);
        offset += grid.x * grid.y * DEFOCUS_CELL_VECTORS;
    }
    return offset;
}

fn defocus_cells_per_slot() -> u32 {
    let grid = defocus_map_grid(DEFOCUS_LOCAL_BINS);
    return defocus_cell_offset(DEFOCUS_BINS) + grid.x * grid.y;
}

// Adds this tile's core to the map's sums of bin `defocus_pass.bin`: per
// cell, the sums of the pixels whose centres it covers; bin 3's cells also
// add the fill's. Export tiles add only their cores, one tile after another,
// so the sums equal the whole image's in any tiling.
@compute @workgroup_size(8, 8, 1)
fn accumulate_defocus_map(@builtin(global_invocation_id) gid: vec3<u32>) {
    let bin = defocus_pass.bin;
    let grid = defocus_map_grid(bin);
    let capacity = arrayLength(&defocus_cells) / max(defocus_cells_per_slot(), 1u);
    let layers = defocus_layer_count(capacity);
    if any(gid.xy >= grid) || gid.z >= layers { return; }
    let span_x = Common::full_image_grid_pixel_span(gid.x, grid.x, Common::camera_uniforms.full_width);
    let span_y = Common::full_image_grid_pixel_span(gid.y, grid.y, Common::camera_uniforms.full_height);
    let bounds = vec4<i32>(Common::camera_uniforms.tone_histogram_bounds);
    let origin = Common::tile_origin();
    let tile_end = origin + vec2<i32>(
        i32(Common::camera_uniforms.width),
        i32(Common::camera_uniforms.height),
    );
    let lo = max(max(vec2<i32>(span_x.x, span_y.x), bounds.xy), origin);
    let hi = min(min(vec2<i32>(span_x.y, span_y.y), bounds.zw), tile_end);
    if any(hi <= lo) { return; }
    let index = defocus_slot(gid.z);
    let params = mask_effect_params(index);
    let centre = 0.5 * vec2<f32>(vec2<i32>(span_x.x + span_x.y, span_y.x + span_y.y)) - vec2<f32>(origin);
    let surround = defocus_surround(centre, defocus_surround_radius(bin));
    var sums = DefocusBinSums(vec4<f32>(0.0), vec4<f32>(0.0), vec4<f32>(0.0), vec4<f32>(0.0));
    var fill = vec4<f32>(0.0);
    // Cells are full / grid pixels wide; pixel centres at x + 0.5.
    let cell = vec2<f32>(
        f32(Common::camera_uniforms.full_width) / f32(grid.x),
        f32(Common::camera_uniforms.full_height) / f32(grid.y),
    );
    let cell_centre = (vec2<f32>(gid.xy) + vec2<f32>(0.5)) * cell;
    for (var y = lo.y; y < hi.y; y = y + 1) {
        for (var x = lo.x; x < hi.x; x = x + 1) {
            let pos = vec2<i32>(x, y) - origin;
            let position = (vec2<f32>(f32(x), f32(y)) + vec2<f32>(0.5) - cell_centre) / cell;
            defocus_add_pixel(&sums, pos, position, index, params, bin, surround);
            if bin == DEFOCUS_LOCAL_BINS {
                fill += defocus_fill_pixel(pos, index, params);
            }
        }
    }
    let slot_offset = gid.z * defocus_cells_per_slot();
    let sums_at = slot_offset + defocus_cell_offset(bin) + (gid.y * grid.x + gid.x) * DEFOCUS_CELL_VECTORS;
    defocus_cells[sums_at] = defocus_cells[sums_at] + sums.front;
    defocus_cells[sums_at + 1u] = defocus_cells[sums_at + 1u] + sums.back;
    defocus_cells[sums_at + 2u] = defocus_cells[sums_at + 2u] + sums.extra;
    defocus_cells[sums_at + 3u] = defocus_cells[sums_at + 3u] + sums.centroid;
    if bin == DEFOCUS_LOCAL_BINS {
        let fill_cell = slot_offset + defocus_cell_offset(DEFOCUS_BINS) + gid.y * grid.x + gid.x;
        defocus_cells[fill_cell] = defocus_cells[fill_cell] + fill;
    }
}

// The map's sources of bin `defocus_pass.bin` (at mip `bin - 3`) from its
// sums, and for bin 3 the fill's first level.
@compute @workgroup_size(8, 8, 1)
fn resolve_defocus_map(@builtin(global_invocation_id) gid: vec3<u32>) {
    let bin = defocus_pass.bin;
    let grid = defocus_map_grid(bin);
    let capacity = textureNumLayers(defocus_sources_out) / (DEFOCUS_LOCAL_BINS * DEFOCUS_SOURCE_LAYERS);
    let layers = defocus_layer_count(capacity);
    if any(gid.xy >= grid) || gid.z >= layers { return; }
    let span_x = Common::full_image_grid_pixel_span(gid.x, grid.x, Common::camera_uniforms.full_width);
    let span_y = Common::full_image_grid_pixel_span(gid.y, grid.y, Common::camera_uniforms.full_height);
    let count = f32(max((span_x.y - span_x.x) * (span_y.y - span_y.x), 1));
    let slot_offset = gid.z * defocus_cells_per_slot();
    let cell = slot_offset + defocus_cell_offset(bin) + (gid.y * grid.x + gid.x) * DEFOCUS_CELL_VECTORS;
    let sums = DefocusBinSums(
        defocus_cells[cell],
        defocus_cells[cell + 1u],
        defocus_cells[cell + 2u],
        defocus_cells[cell + 3u],
    );
    let texel = vec2<i32>(gid.xy);
    let layer = (gid.z * DEFOCUS_LOCAL_BINS + bin - DEFOCUS_LOCAL_BINS) * DEFOCUS_SOURCE_LAYERS;
    defocus_store_sources(texel, layer, sums, count);
    if bin == DEFOCUS_LOCAL_BINS {
        let fill_cell = slot_offset + defocus_cell_offset(DEFOCUS_BINS) + gid.y * grid.x + gid.x;
        textureStore(
            defocus_slabs_out,
            texel,
            gid.z * DEFOCUS_SLAB_LAYERS + DEFOCUS_FILL_LAYER,
            defocus_cells[fill_cell] / count,
        );
    }
}

// --- Composite ---

// Slab `bin` (local) on `side` at tile pixel `pos`, bilinear.
fn defocus_local_slab(pos: vec2<i32>, layer: u32, bin: u32, side: u32) -> vec4<f32> {
    let level = defocus_local_level(bin);
    return defocus_local_sample(pos, layer * DEFOCUS_SLAB_LAYERS + 2u * bin + side, level);
}

fn defocus_local_sample(pos: vec2<i32>, layer: u32, level: u32) -> vec4<f32> {
    let mip = level - defocus_pass.levels.x;
    let size = vec2<f32>(textureDimensions(defocus_slabs_tex, mip));
    let global = vec2<f32>(pos + Common::tile_origin()) + vec2<f32>(0.5);
    let texel = global / f32(1u << level) - vec2<f32>(defocus_level_origin(level));
    return textureSampleLevel(defocus_slabs_tex, SceneAdjustments::local_mask_sampler, texel / size, layer, f32(mip));
}

fn defocus_map_sample(pos: vec2<i32>, layer: u32, mip: u32) -> vec4<f32> {
    return textureSampleLevel(defocus_map_tex, SceneAdjustments::local_mask_sampler, full_image_uv(pos), layer, f32(mip));
}

// The focus slab at tile pixel `pos`: the scatter of its sources within the
// widest radius any lies at around it, normalized by what the front slabs
// leave visible. A source behind the pixel's own focus surface reaches it no
// farther than that surface's radius.
fn defocus_focus_slab(pos: vec2<i32>, layer: u32, aperture: DefocusAperture) -> vec4<f32> {
    let own = defocus_focus_source(pos, layer);
    // The widest radius in the blocks around.
    let block = pos / DEFOCUS_FOCUS_BLOCK;
    let limit = DEFOCUS_FOCUS_RADIUS * DEFOCUS_BIN_REACH * defocus_short_pixels();
    let blocks = i32(ceil(limit / f32(DEFOCUS_FOCUS_BLOCK)));
    var widest = 0u;
    for (var by = -blocks; by <= blocks; by = by + 1) {
        for (var bx = -blocks; bx <= blocks; bx = bx + 1) {
            let neighbour = block + vec2<i32>(bx, by);
            if !defocus_occupancy_valid(neighbour) { continue; }
            widest = max(widest, atomicLoad(&defocus_occupancy[defocus_occupancy_index(layer, 0u, neighbour)]));
        }
    }
    let reach = i32(ceil(min(f32(widest) / DEFOCUS_OCCUPANCY_SCALE, limit)));
    if reach == 0 {
        let coverage = clamp(own.alpha / max(own.visible, 0.05), 0.0, 1.0);
        return vec4<f32>(own.premultiplied * (coverage / max(own.alpha, 1e-9)), select(0.0, coverage, own.alpha > 0.0));
    }
    let point = defocus_point(full_image_uv(pos));
    var colour = vec3<f32>(0.0);
    var scattered = 0.0;
    var visible = 0.0;
    var area = 0.0;
    for (var dy = -reach; dy <= reach; dy = dy + 1) {
        for (var dx = -reach; dx <= reach; dx = dx + 1) {
            let offset = vec2<f32>(f32(dx), f32(dy));
            let distance = length(offset);
            if distance > f32(reach) + 0.5 { continue; }
            let source = defocus_focus_source(pos + vec2<i32>(dx, dy), layer);
            area += 1.0;
            visible += source.visible;
            if source.alpha <= 0.0 { continue; }
            if own.alpha > 0.05
                && source.depth < own.depth * (1.0 - DEFOCUS_DEPTH_TOLERANCE)
                && distance > own.radius + 0.5 {
                continue;
            }
            var weight = 0.0;
            if source.radius < 0.5 {
                weight = select(0.0, 1.0, dx == 0 && dy == 0);
            } else {
                weight = defocus_spread(aperture, -offset, source.radius, point);
            }
            colour += weight * source.premultiplied;
            scattered += weight * source.alpha;
        }
    }
    if scattered <= 0.0 {
        return vec4<f32>(0.0);
    }
    let coverage = clamp(scattered / max(visible / max(area, 1.0), 0.05), 0.0, 1.0);
    return vec4<f32>(colour * (coverage / scattered), coverage);
}

// The fill at tile pixel `pos` for a hole about `radius` pixels wide: the
// first level from about that scale whose non-front share counts, normalized.
fn defocus_fill(pos: vec2<i32>, layer: u32, radius: f32) -> vec3<f32> {
    let fill_layer = layer * DEFOCUS_SLAB_LAYERS + DEFOCUS_FILL_LAYER;
    let local_levels = textureNumLevels(defocus_slabs_tex);
    let wanted = u32(max(ceil(log2(max(radius, 1.0))) + 1.0, 0.0));
    var level = max(wanted, defocus_pass.levels.x);
    var last = vec4<f32>(0.0);
    for (var attempt = 0u; attempt < 4u; attempt = attempt + 1u) {
        if level - defocus_pass.levels.x < local_levels {
            last = defocus_local_sample(pos, fill_layer, level);
        } else if defocus_pass.map_ready != 0u {
            // The map's cells are long / DEFOCUS_MAP_LONG pixels wide.
            let full = max(f32(max(Common::camera_uniforms.full_width, Common::camera_uniforms.full_height)), 1.0);
            let cell = full / f32(DEFOCUS_MAP_LONG);
            let map_levels = textureNumLevels(defocus_map_tex);
            let mip = min(u32(max(ceil(log2(max(f32(1u << level) / cell, 1.0))), 0.0)), map_levels - 1u);
            last = defocus_map_sample(pos, fill_layer, mip);
        }
        if last.w >= DEFOCUS_FILL_WEIGHT {
            break;
        }
        level = level + 1u;
    }
    return last.rgb / max(last.w, 1e-6);
}

// The slabs composited front to back with "over", then the fill for what
// none covers.
@compute @workgroup_size(8, 8, 1)
fn composite_defocus(@builtin(global_invocation_id) gid: vec3<u32>) {
    let size = textureDimensions(defocus_result_out);
    let layers = defocus_layer_count(textureNumLayers(defocus_result_out));
    if any(gid.xy >= size) || gid.z >= layers { return; }
    let pos = vec2<i32>(gid.xy);
    let layer = gid.z;
    let params = mask_effect_params(defocus_slot(layer));
    let aperture = defocus_aperture(params);
    var colour = vec3<f32>(0.0);
    var transmittance = 1.0;
    // The front slabs' radius, weighted by what each shows, for the fill.
    var front_radius = 0.0;
    var front_shown = 0.0;
    // Front bins, widest first.
    for (var step = 0u; step < DEFOCUS_BINS; step = step + 1u) {
        let bin = DEFOCUS_BINS - 1u - step;
        var slab = vec4<f32>(0.0);
        if bin >= DEFOCUS_LOCAL_BINS {
            if defocus_pass.map_ready != 0u {
                slab = defocus_map_sample(pos, layer * DEFOCUS_SLAB_LAYERS + 2u * (bin - DEFOCUS_LOCAL_BINS), defocus_map_slab_mip(bin));
            }
        } else {
            slab = defocus_local_slab(pos, layer, bin, 0u);
        }
        let shown = transmittance * slab.w;
        front_radius += shown * 1.5 * defocus_bin_start(bin);
        front_shown += shown;
        colour += transmittance * slab.rgb;
        transmittance *= 1.0 - clamp(slab.w, 0.0, 1.0);
    }
    let focus = defocus_focus_slab(pos, layer, aperture);
    colour += transmittance * focus.rgb;
    transmittance *= 1.0 - focus.w;
    // Back bins, narrowest first.
    for (var bin = 0u; bin < DEFOCUS_BINS; bin = bin + 1u) {
        var slab = vec4<f32>(0.0);
        if bin >= DEFOCUS_LOCAL_BINS {
            if defocus_pass.map_ready != 0u {
                slab = defocus_map_sample(pos, layer * DEFOCUS_SLAB_LAYERS + 2u * (bin - DEFOCUS_LOCAL_BINS) + 1u, defocus_map_slab_mip(bin));
            }
        } else {
            slab = defocus_local_slab(pos, layer, bin, 1u);
        }
        colour += transmittance * slab.rgb;
        transmittance *= 1.0 - clamp(slab.w, 0.0, 1.0);
    }
    if transmittance > 1e-3 {
        let radius = front_radius / max(front_shown, 1e-6) * defocus_short_pixels();
        colour += transmittance * defocus_fill(pos, layer, radius);
    }
    textureStore(defocus_result_out, pos, layer, vec4<f32>(colour, 1.0));
}
