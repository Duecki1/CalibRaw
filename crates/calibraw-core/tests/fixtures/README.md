# RAW compatibility fixtures

`linearraw-16bit.jxl` is an original synthetic 16 × 16 RGB image. Each pixel
contains the unsigned 16-bit samples `[8192, 16384, 32768]`. It is used inside
generated DNG containers to test JPEG-XL decoding and LinearRaw normalization.
It contains no camera image or third-party content.

Generated with libjxl `cjxl` 0.12.0 from a binary, big-endian 16-bit PPM:

```sh
cjxl input.ppm linearraw-16bit.jxl --distance=0 --effort=3
```

The fixture is distributed under the repository's GPL-3.0-or-later license.
Running the tests does not require `cjxl`.
