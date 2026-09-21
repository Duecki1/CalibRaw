# Black-tone processing

## Verified issue and implementation

The display Blacks control is implemented by
`apply_display_blacks_toe_amount` in
`crates/calibraw-gpu/src/shaders/tonemap.wgsl`. The previous negative branch
used a deep-crush term of up to `10.5 EV`, gated by
`smoothstep(0.012, 0.030, Y)`, plus a decaying tail. Its local slope reached
approximately `4.93x` at `-100` and `2.37x` at `-10` in the affected encoded
shadow cases. This amplified existing shadow noise into the coarse,
pixelated appearance seen when Blacks was changed.

The negative branch now uses the bounded offset

```text
offset_ev = amount * 4 / (1 + Y / 0.035) * hdr_guard
```

where `amount` is negative for crush. The resulting gain is applied once to
all RGB channels, preserving their ratios. This is a ratio-preserving RGB
operation, not a universal perceptual-hue guarantee. The branch keeps the
existing HDR fade, `1 - smoothstep(0.35, 1.0, Y)`, and the `Y <= 0` guard used
by the valid finite pipeline inputs.

The positive Blacks curve is unchanged. The bounded negative response is
monotonic and has a finite black endpoint. GPU regression tests bound the
measured encoded negative-shadow noise slope to `1.2x` for neutral sRGB ramps
in the shadow domain `Y <= 0.15`, and the overall response to `1.3x` across
the full `Y` domain `[0, 1]`. These are acceptance bounds, not mathematical
limits for arbitrary colors or profiles. The fabric integration check retains
the tighter `1.1x` bound at its sampled shadow points `Y = 0.012`, `0.022`,
and `0.026`.
Global and local masks use the same helper: the global value path is called by
`view_transform.wgsl`, and local mask amounts call the amount helper directly.

Existing edits with negative Blacks values render differently because the
negative curve changed. They may need readjustment; the settings schema is
unchanged.

No spatial blur, denoising, or default sharpening change is part of this fix.
Those operations would alter genuine detail and conceal the tone-curve gain;
the observed symptom is addressed at the amplification source.

## Validation and related references

The direct shader harness imports the production module through `ShaderManager`
and uses the real slider wrapper. It checks neutral ramps and colored patches
for monotonic luminance, channel-ratio preservation, black-endpoint behavior,
HDR fade, and the domain-specific response limits. A separate pipeline test
checks noisy shadow patches, global/local equivalence, and preview/export
agreement. Restoring the old branch makes the slope regression fail at `-10`.

Run the focused regressions with `cargo test -p calibraw-gpu --lib black`.

This bounded toe follows the same general concern documented by
[darktable filmic RGB](https://docs.darktable.org/usermanual/development/en/module-reference/processing-modules/filmic-rgb/),
which describes smooth transitions at the shadow end and warns that black
measurements are sensitive to noise. [RawPedia Exposure](https://rawpedia.rawtherapee.com/Exposure)
similarly treats Black as a black-point control and Shadow Compression as a
way to dampen its effect, supporting a bounded shadow response rather than
deep unrestricted amplification.
