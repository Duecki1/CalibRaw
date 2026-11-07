//! The parameters of an effect slot as its shader reads them.
//!
//! An effect packs up to twelve values into lanes 0..12 and per-slot switches
//! into four options (`MaskEffectParams` in mask_effects/effect_api.wgsl). Each
//! effect names its lanes with [`effect_lanes!`] in its settings module;
//! calibraw-gpu writes those names into the shaders as `<EFFECT>_<NAME>_LANE`
//! and the options as `<NAME>_OPTION`, so a shader that reads its slot by these
//! names cannot drift apart from the packing. Every effect's shader does,
//! except Sharpen's: its slot has the layout of the Detail card's sharpening,
//! which detail_capture.wgsl reads by position (sharpen.rs).

/// Lanes in an effect slot: `adjust_0`..`adjust_2` of `MaskData`, four each.
const LANE_COUNT: usize = 12;
/// Per-slot switches (`effect_options` of `MaskData`).
pub const OPTION_COUNT: usize = 4;

/// One lane of an effect slot.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Lane(usize);

impl Lane {
    pub const fn new(index: usize) -> Self {
        assert!(index < LANE_COUNT, "an effect lane lies past the slot");
        Self(index)
    }

    pub const fn index(self) -> usize {
        self.0
    }
}

/// Three consecutive lanes that hold a colour.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ColorLane(usize);

impl ColorLane {
    pub const fn new(index: usize) -> Self {
        assert!(
            index + 3 <= LANE_COUNT,
            "a colour lane reaches past the slot"
        );
        Self(index)
    }

    pub const fn index(self) -> usize {
        self.0
    }
}

/// A per-slot switch.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EffectOption(usize);

impl EffectOption {
    const fn new(index: usize) -> Self {
        assert!(index < OPTION_COUNT, "an effect option lies past the slot");
        Self(index)
    }

    pub const fn index(self) -> usize {
        self.0
    }
}

/// The options of every effect. They are declared together because effects
/// share some (Image lights) and no two may collide.
pub mod options {
    use super::EffectOption;

    /// Rain: 1 when streaks and mist follow scene depth.
    pub const RAIN_DEPTH: EffectOption = EffectOption::new(1);
    /// Fog, Smoke and Rain: 1 when Image lights is on.
    pub const MEDIUM_IMAGE_LIGHTS: EffectOption = EffectOption::new(3);

    /// Every option as (name, index), for the shaders.
    pub const ALL: [(&str, usize); 2] = [
        ("RAIN_DEPTH", RAIN_DEPTH.index()),
        ("MEDIUM_IMAGE_LIGHTS", MEDIUM_IMAGE_LIGHTS.index()),
    ];
}

/// A named lane as the shaders see it: its name, first lane and width.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LaneName {
    pub name: &'static str,
    pub index: usize,
    pub width: usize,
}

/// Fails to compile when two of `lanes` overlap. Each lane is already checked
/// to lie inside the slot when it is declared.
pub(super) const fn assert_disjoint(lanes: &[LaneName]) {
    let mut occupied = [false; LANE_COUNT];
    let mut entry = 0;
    while entry < lanes.len() {
        let mut lane = lanes[entry].index;
        while lane < lanes[entry].index + lanes[entry].width {
            assert!(!occupied[lane], "two effect lanes overlap");
            occupied[lane] = true;
            lane += 1;
        }
        entry += 1;
    }
}

/// Declares an effect's lanes as `pub mod lanes`: `NAME = index` for a value,
/// `NAME = index (color)` for the three lanes of a colour. The names are those
/// the shader reads (`<EFFECT>_<NAME>_LANE`).
macro_rules! effect_lanes {
    ($($(#[$doc:meta])* $name:ident = $index:literal $(($color:ident))?),+ $(,)?) => {
        /// The lanes this effect's shader reads.
        pub mod lanes {
            use $crate::pipeline::masks::effects::lanes::{assert_disjoint, effect_lanes, LaneName};

            $(effect_lanes!(@lane $(#[$doc])* $name = $index $(($color))?);)+

            /// Every lane, for the shaders.
            pub const ALL: &[LaneName] = &[
                $(LaneName {
                    name: stringify!($name),
                    index: $index,
                    width: effect_lanes!(@width $(($color))?),
                },)+
            ];
            const _: () = assert_disjoint(ALL);
        }
    };
    (@lane $(#[$doc:meta])* $name:ident = $index:literal) => {
        $(#[$doc])*
        pub const $name: $crate::pipeline::masks::effects::lanes::Lane =
            $crate::pipeline::masks::effects::lanes::Lane::new($index);
    };
    (@lane $(#[$doc:meta])* $name:ident = $index:literal (color)) => {
        $(#[$doc])*
        pub const $name: $crate::pipeline::masks::effects::lanes::ColorLane =
            $crate::pipeline::masks::effects::lanes::ColorLane::new($index);
    };
    (@width) => {
        1
    };
    (@width (color)) => {
        3
    };
}
pub(super) use effect_lanes;

/// An effect slot's packed parameters: what its shader reads, before
/// calibraw-gpu places it in `MaskData`.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct EffectLanes {
    lanes: [f32; LANE_COUNT],
    options: [f32; OPTION_COUNT],
}

impl EffectLanes {
    pub fn set(&mut self, lane: Lane, value: f32) {
        self.lanes[lane.index()] = value;
    }

    pub fn set_color(&mut self, lane: ColorLane, color: [f32; 3]) {
        self.lanes[lane.index()..lane.index() + 3].copy_from_slice(&color);
    }

    pub fn set_option(&mut self, option: EffectOption, value: f32) {
        self.options[option.index()] = value;
    }

    /// Sets a switch to 1 when `on`, else 0.
    pub fn set_switch(&mut self, option: EffectOption, on: bool) {
        self.set_option(option, f32::from(u8::from(on)));
    }

    /// The lanes in rows of four, as `adjust_0`..`adjust_2`.
    pub fn rows(&self) -> [[f32; 4]; 3] {
        let row = |start: usize| {
            [
                self.lanes[start],
                self.lanes[start + 1],
                self.lanes[start + 2],
                self.lanes[start + 3],
            ]
        };
        [row(0), row(4), row(8)]
    }

    pub fn options(&self) -> [f32; OPTION_COUNT] {
        self.options
    }
}

/// What packing knows besides the settings.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PackContext {
    /// The photo's exposure time, or 0 when it records none.
    pub shutter_seconds: f32,
    /// The instance the slot holds, below `EffectSettings::instance_count`;
    /// 0 for an effect with one.
    pub instance: usize,
}
