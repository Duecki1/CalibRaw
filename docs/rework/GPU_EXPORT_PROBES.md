# Export and native compiler isolation probes

These probes retain the original pixel values and do not change saved edits.
Enable only the comparison being investigated. Stage captures insert waits;
`observe` keeps the normal export sequence until a non-finite readback occurs.

## Samsung last-tile investigation

Create `export-probe` beside the private `session.log`, under `diagnostics`.
On Android, with the debuggable app installed:

```sh
printf observe | adb shell run-as de.duecki.calibraw tee files/diagnostics/export-probe > /dev/null
```

The file is read for each export. Its contents select one mode:

| Contents | Behaviour |
| --- | --- |
| `raw` | Develop only the last tile, at its original global coordinates and with its original padded size; capture RAW scene and stop without publishing an image |
| `stages` | Run the original whole-image analysis and effect-map prepasses, then develop the last tile first; capture RAW scene, scene preparation, tone output and display output, then copy the exact production core rectangle and stop |
| `observe` | Export in the normal order with pipelined readback; on non-finite RGB, retain the currently held GPU textures without rerendering |
| `sync` | Export in the normal order, finishing each core readback before rendering the next tile; retain failure textures as in `observe` |

Delete the marker to restore ordinary export:

```sh
adb shell run-as de.duecki.calibraw rm files/diagnostics/export-probe
```

`raw` and `stages` intentionally finish with a diagnostic-complete export
message and no published image. They are not successful-export acceptance
tests. `sync` changes readback pacing; a success there must be compared with
`observe`, rather than being treated as a shader repair.

Captures live beside the journal:

- `export-probe-capture/`: isolated stages, `parameters.txt`, packed parameter
  buffers, readable uniform fields, and whole-image tone statistics.
- `invalid-readback.rgb32f` and `.txt`: the actual RGB rejected by a completed
  production readback, its dimensions and stage. Retained on finite-check
  failure even when the device remains valid. Wait/map errors remain separate.
- `failed-export-capture/`: snapshots after a finite-check failure in an
  observed/synchronous export. `raw-scene`, `work-1`, `work-2` and
  `display-linear` describe the current GPU textures. With pipelining the
  current GPU tile can be newer than the failed readback; both tile indices
  are logged. No stage is recomputed before these captures.

Image files contain unpadded row-major RGB, little-endian f32, with no header.
Full texture dimensions and the local core rectangle are in the journal and
`parameters.txt`; `display-core.rgb32f` uses the core's dimensions. Summaries
count non-finite channel values in the core separately from the whole texture
and identify the first core pixel/channel and raw bits. Every stage also copies
the exact core rectangle separately and compares each channel bit-for-bit
with the corresponding full-texture texel; both transfers are retained. Compare
invalid data with the corresponding rectangle of `display-linear` to distinguish
a bad rendered pixel from a copy/mapping problem.

Each extra texture readback is limited to the existing 64 MiB readback bound
and reserves its GPU bytes. Larger diagnostic textures are declined, rather
than adding an unbounded allocation to a failing job. Production validation
continues to reject non-finite pixels; no capture replaces them with zero.

The readback extraction is shared with the diagnostic consumer so copying,
row-padding removal and byte decoding have one implementation. The diagnostic
module owns the retained raw pixels and their summaries. These are runtime
diagnostics, not a new test harness or a test-only rendering API.

## Glow/native compilation comparison

Create `gpu-compile-probe` beside the private journal **before launching the
process**, or set `CALIBRAW_GPU_COMPILE_PROBE` on desktop/CLI. The setting is
read once and its active options are journalled. Desktop journals live under
the performance-settings directory: `%APPDATA%/calibraw/diagnostics` on Windows,
`~/.config/calibraw/diagnostics` on Linux unless `XDG_CONFIG_HOME` is set.

Options may be combined with spaces:

| Option | Comparison |
| --- | --- |
| `generic` | Use the program containing every effect; do not start specialized effect-set compilation |
| `specialized` | Wait for the requested effect-set program, avoiding the generic program's first-use fallback while it compiles |
| `serial` | Serialize processing-pipeline native compilation across its workers |
| `no-cache` | Pass no application pipeline cache to compute-pipeline creation; preserve the existing cache file |

For example:

```sh
printf 'specialized serial no-cache' | adb shell run-as de.duecki.calibraw tee files/diagnostics/gpu-compile-probe > /dev/null
```

Run generic-only and specialized-only separately first, then compare serial
and default overlap with the same photo and driver. Specialized mode can still
reach the generic fallback if specialization itself returns a recoverable
compile error; native crashes cannot be caught by Rust unwinding. Inspect
begin/end records and effect-set labels rather than assuming which program
ran. Serialization covers CalibRaw processing programs, not unrelated egui or
driver-internal compilation. `no-cache` does not clear a driver's own cache.

Delete the file/unset the variable and restart to restore default behaviour.
Keep the exact APK/executable and native Build ID with every capture. Linux
parity can verify output compatibility of these modes, but cannot qualify a
PowerVR or Windows AMD native compiler fix.
