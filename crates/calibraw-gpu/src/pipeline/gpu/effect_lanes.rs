//! Named parameter lanes of effect slots in `MaskData`.
//!
//! An effect slot packs up to twelve parameters into `adjust_0`..`adjust_2`:
//! lane `n` is component `n % 4` of `adjust_{n / 4}`. A colour takes three
//! consecutive lanes. Per-slot switches live in `effect_options` (`options`).
//!
//! The shaders read the same lanes by name: `<EFFECT>_<NAME>_LANE` and
//! `<NAME>_OPTION` constants with `mask_effect_lane` and friends
//! (mask_effects/shared.wgsl). `layout_contract_tests` checks every name here
//! against them, so packing and reading cannot drift apart. Effects not named
//! here still pack positionally (`pack_effect_mask`).

macro_rules! effect_lanes {
    ($($effect:ident { $($name:ident = $lane:literal),+ $(,)? })+) => {
        $(
            pub(super) mod $effect {
                $(pub(in crate::pipeline::gpu) const $name: usize = $lane;)+
                // `MaskData::set_lane` relies on every lane being in `adjust_0..2`.
                $(const _: () = assert!($name < 12);)+
            }
        )+

        /// Every named lane as (effect, name, lane).
        #[cfg(test)]
        pub(super) fn named_lanes() -> Vec<(&'static str, &'static str, usize)> {
            vec![$($((stringify!($effect), stringify!($name), $effect::$name),)+)+]
        }
    };
}

effect_lanes! {
    relight {
        AMOUNT = 0,
        REACH = 1,
        SOURCE_X = 2,
        SOURCE_Y = 3,
        COLOR = 4,
        DEPTH = 7,
        SIZE = 8,
        SHADOWS = 9,
        RELIEF = 10,
        AMBIENT = 11,
    }
    light_rays {
        AMOUNT = 0,
        LENGTH = 1,
        SOURCE_X = 2,
        SOURCE_Y = 3,
        COLOR = 4,
        FADE = 7,
        SPREAD = 8,
        RAY_COUNT = 9,
        VARIATION = 10,
        SOFTNESS = 11,
    }
    fog {
        AMOUNT = 0,
        DENSITY = 1,
        SCALE = 2,
        SOFTNESS = 3,
        COLOR = 4,
        VARIATION = 7,
        SEED = 8,
        START = 9,
        DEPTH_INFLUENCE = 10,
        LIGHT_GLOW = 11,
    }
    smoke {
        AMOUNT = 0,
        DENSITY = 1,
        SCALE = 2,
        TURBULENCE = 3,
        COLOR = 4,
        ANGLE = 7,
        SOFTNESS = 8,
        SEED = 9,
        LIGHT_GLOW = 10,
    }
}

/// `effect_options` lanes of effect slots.
pub(super) mod options {
    /// Fog and Smoke: 1 when Image lights is on.
    pub(in crate::pipeline::gpu) const MEDIUM_IMAGE_LIGHTS: usize = 3;
    /// Relight: its shadow-map channel plus one, or 0 to trace shadows per
    /// pixel (`assign_relight_shadow_channels`).
    pub(in crate::pipeline::gpu) const RELIGHT_SHADOW_CHANNEL: usize = 2;

    /// Every option as (name, lane).
    #[cfg(test)]
    pub(in crate::pipeline::gpu) fn named_options() -> Vec<(&'static str, usize)> {
        vec![
            ("MEDIUM_IMAGE_LIGHTS", MEDIUM_IMAGE_LIGHTS),
            ("RELIGHT_SHADOW_CHANNEL", RELIGHT_SHADOW_CHANNEL),
        ]
    }
}

/// Lanes a parameter occupies: three for a colour, one otherwise.
#[cfg(test)]
pub(super) fn lane_width(name: &str) -> usize {
    if name == "COLOR" {
        3
    } else {
        1
    }
}

impl super::MaskData {
    fn lanes(&self) -> [&[f32; 4]; 3] {
        [&self.adjust_0, &self.adjust_1, &self.adjust_2]
    }

    pub(super) fn lane(&self, lane: usize) -> f32 {
        self.lanes()[lane / 4][lane % 4]
    }

    pub(super) fn set_lane(&mut self, lane: usize, value: f32) {
        // Named lanes are below 12, checked where they are declared.
        let row = match lane / 4 {
            0 => &mut self.adjust_0,
            1 => &mut self.adjust_1,
            _ => &mut self.adjust_2,
        };
        row[lane % 4] = value;
    }

    /// Sets the three lanes of a colour starting at `lane`.
    pub(super) fn set_color_lanes(&mut self, lane: usize, color: [f32; 3]) {
        for (offset, value) in color.into_iter().enumerate() {
            self.set_lane(lane + offset, value);
        }
    }
}
