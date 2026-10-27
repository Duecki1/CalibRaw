# AI parity goldens

Reference outputs of `crates/calibraw-ai/tests/parity.rs`, from CPU sessions.
`SHA256SUMS` pins every file. Do not edit by hand; bless them again with
`CALIBRAW_PARITY_BLESS=1` (see [AI parity](../../docs/DEVELOPMENT.md#ai-parity)).

| Files | Content |
|---|---|
| `subject-*.png`, `sky.png`, `object*.png`, `people-*.png` | Probability masks, 8-bit grayscale |
| `depth.png` | Relative depth, 8-bit grayscale |
| `denoise-bayer.png` | Denoised RAW sensor codes, 16-bit grayscale |
| `denoise-xtrans.png` | Camera RGB; 16-bit samples are IEEE float16 bit patterns |
| `remove.png`, `remove-alpha.png`, `remove-origin.txt` | The Remove patch as float16 RGB, its alpha, and its position in the crop |

`remove*` and `denoise-xtrans.png` depend on GPU rendering or demosaicing
before inference and are specific to the adapter that blessed them.
