//! White balance of a loaded RAW: as-shot values, temperature/tint and presets.

use super::*;

impl ProfileColor {
    pub fn as_shot_white_balance(&self) -> Option<(f32, f32)> {
        #[cfg(libraw_available)]
        {
            Some(libraw_loader::as_shot_temperature_tint(
                self.white_balance_model.as_ref()?,
            ))
        }
        #[cfg(not(libraw_available))]
        {
            None
        }
    }

    /// Fixed temperature and tint slider ranges for this colour.
    pub fn white_balance_ranges(&self) -> Option<WhiteBalanceRanges> {
        #[cfg(libraw_available)]
        {
            libraw_loader::white_balance_ranges(self.white_balance_model.as_ref()?)
        }
        #[cfg(not(libraw_available))]
        {
            None
        }
    }

    /// Temperature and tint the sliders show for stored offsets, each limited
    /// to its own slider range.
    pub fn white_balance_temperature_tint(
        &self,
        temperature_offset: f32,
        tint_offset: f32,
    ) -> Option<(f32, f32)> {
        let (base_temperature, base_tint) = self.as_shot_white_balance()?;
        Some(self.white_balance_ranges()?.clamp(
            temperature_kelvin_from_offset(base_temperature, temperature_offset),
            white_balance_tint_from_offset(base_tint, tint_offset),
        ))
    }

    /// Stored offsets for a temperature and tint, each first limited to its
    /// own slider range.
    pub fn white_balance_offsets_from_temperature_tint(
        &self,
        temperature: f32,
        tint: f32,
    ) -> Option<(f32, f32)> {
        let (base_temperature, base_tint) = self.as_shot_white_balance()?;
        let (temperature, tint) = self.white_balance_ranges()?.clamp(temperature, tint);
        Some((
            temperature_offset_from_kelvin(base_temperature, temperature),
            white_balance_tint_offset(base_tint, tint),
        ))
    }

    /// Temperature and tint offsets from the as-shot white balance that
    /// render `white_balance` with this colour, as the sensor multipliers
    /// take them. Zero, so as shot, without a white-balance model.
    pub fn white_balance_offsets(&self, white_balance: WhiteBalance) -> (f32, f32) {
        match white_balance {
            WhiteBalance::AsShot => (0.0, 0.0),
            WhiteBalance::Custom { kelvin, tint, .. } => self
                .white_balance_offsets_from_temperature_tint(kelvin, tint)
                .unwrap_or((0.0, 0.0)),
            // migration: remove in v2.0.0
            WhiteBalance::LegacyOffset { temperature, tint } => (temperature, tint),
        }
    }

    /// White-balance multipliers, camera-to-working transform and
    /// hue/saturation table weight for temperature and tint offsets from
    /// `as_shot`, the photo's as-shot multipliers. As the DNG SDK renders a
    /// white, a dual-illuminant profile's matrices and table weight are
    /// interpolated at the white's own temperature; single-matrix models keep
    /// the as-shot transform and weight. The as-shot white returns the values
    /// the photo loaded with.
    pub fn adjusted_white_balance_and_camera_transform(
        &self,
        as_shot: [f32; 4],
        temperature: f32,
        tint: f32,
    ) -> ([f32; 4], [[f32; 4]; 3], f32) {
        let as_shot = (
            as_shot,
            self.cam_to_srgb,
            self.camera_profile.interpolation_weight,
        );
        if temperature.abs() < 1e-6 && tint.abs() < 1e-6 {
            return as_shot;
        }
        #[cfg(libraw_available)]
        if let Some(model) = &self.white_balance_model {
            if let Some(adjusted) = libraw_loader::adjusted_white_balance(
                model,
                temperature.clamp(-GLOBAL_TEMPERATURE_LIMIT, GLOBAL_TEMPERATURE_LIMIT),
                tint.clamp(-GLOBAL_TINT_OFFSET_LIMIT, GLOBAL_TINT_OFFSET_LIMIT),
                self.camera_profile.interpolation_weight,
            ) {
                let (transform, weight) = adjusted.profile.unwrap_or((as_shot.1, as_shot.2));
                return (adjusted.coefficients, transform, weight);
            }
        }
        as_shot
    }
}

impl LoadedRaw {
    pub fn as_shot_temperature_kelvin(&self) -> Option<f32> {
        self.as_shot_white_balance().map(|value| value.0)
    }

    pub fn as_shot_white_balance(&self) -> Option<(f32, f32)> {
        self.profile_color.as_shot_white_balance()
    }

    /// Fixed temperature and tint slider ranges for this image.
    pub fn white_balance_ranges(&self) -> Option<WhiteBalanceRanges> {
        self.profile_color.white_balance_ranges()
    }

    /// Temperature and tint the sliders show for stored offsets, each limited
    /// to its own slider range.
    pub fn white_balance_temperature_tint(
        &self,
        temperature_offset: f32,
        tint_offset: f32,
    ) -> Option<(f32, f32)> {
        self.profile_color
            .white_balance_temperature_tint(temperature_offset, tint_offset)
    }

    /// Stored offsets for a temperature and tint, each first limited to its
    /// own slider range.
    pub fn white_balance_offsets_from_temperature_tint(
        &self,
        temperature: f32,
        tint: f32,
    ) -> Option<(f32, f32)> {
        self.profile_color
            .white_balance_offsets_from_temperature_tint(temperature, tint)
    }

    /// Temperature and tint offsets from the as-shot white balance that
    /// render `white_balance` on this photo, as the sensor multipliers take
    /// them. Zero, so as shot, when the photo has no white-balance model.
    pub fn white_balance_offsets(&self, white_balance: WhiteBalance) -> (f32, f32) {
        self.profile_color.white_balance_offsets(white_balance)
    }

    /// Temperature and tint the sliders show for `white_balance`, each limited
    /// to its own slider range.
    pub fn shown_white_balance(&self, white_balance: WhiteBalance) -> Option<(f32, f32)> {
        match white_balance {
            WhiteBalance::AsShot => self.white_balance_temperature_tint(0.0, 0.0),
            WhiteBalance::Custom { kelvin, tint, .. } => {
                Some(self.white_balance_ranges()?.clamp(kelvin, tint))
            }
            // migration: remove in v2.0.0
            WhiteBalance::LegacyOffset { temperature, tint } => {
                self.white_balance_temperature_tint(temperature, tint)
            }
        }
    }

    /// A custom white balance of `kelvin` and `tint`, each first limited to
    /// its own slider range.
    pub fn custom_white_balance(&self, kelvin: f32, tint: f32) -> Option<WhiteBalance> {
        let (kelvin, tint) = self.white_balance_ranges()?.clamp(kelvin, tint);
        let (temperature_offset, tint_offset) =
            self.white_balance_offsets_from_temperature_tint(kelvin, tint)?;
        Some(WhiteBalance::Custom {
            kelvin,
            tint,
            older_reader_offsets: [temperature_offset, tint_offset],
        })
    }

    /// The custom white balance that stored offsets give this photo.
    pub fn white_balance_from_offsets(
        &self,
        (temperature_offset, tint_offset): (f32, f32),
    ) -> Option<WhiteBalance> {
        let (kelvin, tint) =
            self.white_balance_temperature_tint(temperature_offset, tint_offset)?;
        self.custom_white_balance(kelvin, tint)
    }

    /// `white_balance` as this photo keeps it: as shot without a
    /// white-balance model, whose photos render none, and a custom white
    /// balance limited to this camera's slider ranges, with offsets for older
    /// readers from this photo. Unchanged when the ranges are unknown.
    pub fn resolve_white_balance(&self, white_balance: WhiteBalance) -> WhiteBalance {
        if !self.has_white_balance_model() {
            return WhiteBalance::AsShot;
        }
        match white_balance {
            WhiteBalance::AsShot => WhiteBalance::AsShot,
            WhiteBalance::Custom { kelvin, tint, .. } => self
                .custom_white_balance(kelvin, tint)
                .unwrap_or(white_balance),
            // migration: remove in v2.0.0. Offsets saved up to v1.3 become
            // the white balance they rendered on this photo.
            WhiteBalance::LegacyOffset { temperature, tint } => self
                .white_balance_from_offsets((temperature, tint))
                .unwrap_or(white_balance),
        }
    }

    /// Stored offsets for the camera reference white, D65.
    pub fn reference_white_balance_offsets(&self) -> Option<(f32, f32)> {
        #[cfg(libraw_available)]
        {
            let (temperature, tint) =
                libraw_loader::xyz_temperature_tint(crate::color_math::D65_XYZ)?;
            self.white_balance_offsets_from_temperature_tint(temperature, tint)
        }
        #[cfg(not(libraw_available))]
        {
            None
        }
    }

    pub fn camera_white_balance_presets(&self) -> Vec<WhiteBalancePreset> {
        crate::pipeline::white_balance_presets::for_camera(&self.camera_make, &self.camera_model)
    }

    fn physical_white_balance_coefficients(&self, logical: [f32; 4]) -> Option<[f32; 4]> {
        let model = self.profile_color.white_balance_model.as_ref()?;
        let mut physical = model.base_wb;
        for (index, descriptor) in model.cdesc.iter().enumerate() {
            physical[index] = match *descriptor as char {
                'R' | 'r' => logical[0],
                'G' | 'g' => logical[1],
                'B' | 'b' => logical[2],
                _ => model.base_wb[index],
            };
        }
        Some(physical)
    }

    pub fn white_balance_offsets_from_coefficients(
        &self,
        logical_coefficients: [f32; 4],
    ) -> Option<(f32, f32)> {
        #[cfg(libraw_available)]
        {
            let model = self.profile_color.white_balance_model.as_ref()?;
            let physical = self.physical_white_balance_coefficients(logical_coefficients)?;
            let (temperature, tint) =
                libraw_loader::temperature_tint_from_coefficients(&model.color, physical)?;
            self.white_balance_offsets_from_temperature_tint(temperature, tint)
        }
        #[cfg(not(libraw_available))]
        {
            let _ = logical_coefficients;
            None
        }
    }

    pub fn white_balance_offsets_from_area(
        &self,
        first: [f32; 2],
        second: [f32; 2],
        black_point: f32,
    ) -> Option<(f32, f32)> {
        if self.is_pre_demosaiced_raster() {
            return None;
        }
        if self.width == 0 || self.height == 0 {
            return None;
        }
        let mut min = [first[0].min(second[0]), first[1].min(second[1])];
        let mut max = [first[0].max(second[0]), first[1].max(second[1])];
        let minimum_u = (12.0 / self.width as f32).min(0.08);
        let minimum_v = (12.0 / self.height as f32).min(0.08);
        if max[0] - min[0] < minimum_u {
            let center = 0.5 * (min[0] + max[0]);
            min[0] = center - 0.5 * minimum_u;
            max[0] = center + 0.5 * minimum_u;
        }
        if max[1] - min[1] < minimum_v {
            let center = 0.5 * (min[1] + max[1]);
            min[1] = center - 0.5 * minimum_v;
            max[1] = center + 0.5 * minimum_v;
        }
        min = min.map(|value| value.clamp(0.0, 1.0));
        max = max.map(|value| value.clamp(0.0, 1.0));
        let x0 = (min[0] * self.width as f32).floor() as u32;
        let y0 = (min[1] * self.height as f32).floor() as u32;
        let x1 = ((max[0] * self.width as f32).ceil() as u32)
            .max(x0 + 1)
            .min(self.width);
        let y1 = ((max[1] * self.height as f32).ceil() as u32)
            .max(y0 + 1)
            .min(self.height);

        const MAX_PICKER_SAMPLES: f64 = 262_144.0;
        let area_pixels = f64::from(x1 - x0) * f64::from(y1 - y0);
        let mut stride = (area_pixels / MAX_PICKER_SAMPLES).sqrt().ceil().max(1.0) as usize;
        while stride.is_multiple_of(2) || stride.is_multiple_of(3) {
            stride += 1;
        }

        let mut sums = [0.0f64; 4];
        let mut counts = [0u64; 4];
        for y in (y0..y1).step_by(stride) {
            for x in (x0..x1).step_by(stride) {
                let index = y as usize * self.width as usize + x as usize;
                let channel = usize::from(self.color_indices[index].min(3));
                let metadata_black = self.black_levels_per_pixel[index];
                let white = self.white_levels[channel].max(metadata_black + 1.0);
                let sensor_range = (white - metadata_black).max(1.0);
                let calibrated_black = (metadata_black
                    + black_point.clamp(-0.25, 0.25) * sensor_range)
                    .clamp(0.0, white - 1.0);
                let value = (f32::from(self.raw_pixels[index]) - calibrated_black)
                    / (white - calibrated_black);
                if value.is_finite() && (0.001..0.98).contains(&value) {
                    sums[channel] += f64::from(value);
                    counts[channel] += 1;
                }
            }
        }
        let mean = |channel: usize| {
            (counts[channel] > 0).then(|| (sums[channel] / counts[channel] as f64) as f32)
        };
        let red = mean(0)?;
        let blue = mean(2)?;
        let greens = [mean(1), mean(3)].into_iter().flatten().collect::<Vec<_>>();
        if greens.is_empty() {
            return None;
        }
        let green = greens.iter().sum::<f32>() / greens.len() as f32;
        if red <= 1e-6 || green <= 1e-6 || blue <= 1e-6 {
            return None;
        }
        self.white_balance_offsets_from_coefficients([green / red, 1.0, green / blue, 1.0])
    }

    pub fn rawnind_daylight_white_balance(&self) -> [f32; 3] {
        #[cfg(libraw_available)]
        if let Some(model) = &self.profile_color.white_balance_model {
            if let Some(daylight) = libraw_loader::daylight_white_balance(model) {
                return daylight;
            }
        }

        let green = [self.wb_coeffs[1], self.wb_coeffs[3]]
            .into_iter()
            .filter(|value| value.is_finite() && *value > 0.0)
            .fold((0.0, 0u32), |(sum, count), value| (sum + value, count + 1));
        let green = if green.1 > 0 {
            green.0 / green.1 as f32
        } else {
            1.0
        };
        let normalize = |value: f32| {
            let value = value / green.max(1e-8);
            if value.is_finite() && value > 0.0 {
                value
            } else {
                1.0
            }
        };
        [
            normalize(self.wb_coeffs[0]),
            1.0,
            normalize(self.wb_coeffs[2]),
        ]
    }

    /// White-balance multipliers, camera-to-working transform and
    /// hue/saturation table weight for temperature and tint offsets from the
    /// as-shot white ([`ProfileColor::adjusted_white_balance_and_camera_transform`]).
    pub fn adjusted_white_balance_and_camera_transform(
        &self,
        temperature: f32,
        tint: f32,
    ) -> ([f32; 4], [[f32; 4]; 3], f32) {
        self.profile_color
            .adjusted_white_balance_and_camera_transform(self.wb_coeffs, temperature, tint)
    }
}
