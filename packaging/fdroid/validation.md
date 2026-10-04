# Local F-Droid validation

Validated on 2026-10-04 with fdroidserver revision
`c21c177ff6d813697aaf9c988ca9fbb2b571b468` (reported version 2.4.2).

The recipe builds CalibRaw `v1.1.1`, source commit
`60cc0005cc6cc0479bb2c8db8a7b963554f7a393`, and imports the Fastlane listing
from upstream commit `7bde63b1861bf40e00e1009fed1cc8f51bcd6c49`.

## Results

- `fdroid lint de.duecki.calibraw`: passed.
- F-Droid's full local build, including source cleanup, signing configuration
  removal, source scanning, native compilation, Rust compilation and Gradle
  APK packaging: passed.
- APK scan for known non-free classes and extra signing blocks: passed.
- `cargo xtask verify-android-16kb`: passed on the APK produced by F-Droid.
  All packaged native libraries satisfy the ELF alignment check, and ZIP
  alignment passes.
- APK identity: `de.duecki.calibraw`, version `1.1.1`, code `1000707`,
  ARM64 only, target SDK 36.
- Maven Central ONNX Runtime AAR SHA-256 verification: passed.
- Automatic update detection: tested with F-Droid's real `check_tags` routine
  against an isolated local Git fixture. It skipped older tags without the
  version-code field and detected simulated `v1.1.2` / `1000708` and
  `v1.2.0` / `1000709` releases, selecting the higher code correctly.
- Cargo's locked metadata resolves with the added `fdroid_version_code` field.
- Recipe shell syntax, YAML parsing and LF line endings: passed.

Build command:

```sh
fdroid build --latest --no-tarball --no-refresh --stop --scan-binary de.duecki.calibraw
```

The local build uses the installed Android SDK/NDK and Gradle 8.11.1, plus a
temporary Python environment for fdroidserver. The build-server `sudo` package
installation block is skipped for a local build; equivalent host dependencies
were available. A Cargo cache outside the source checkout was retained for the
final run. Source and APK scans were enabled; no force or skip-scan flag was used.

## Artifact

Temporary unsigned APK:
`/tmp/calibraw-fdroid/build-work/unsigned/de.duecki.calibraw_1000707.apk`

SHA-256:
`08d75b38f439ec636916e178d10131fa9d5ef9e2e44fbd865a14878f7ccb8d0e`

## Remaining external checks

The GitLab build-server pipeline and F-Droid's inclusion review have not run.
No merge request has been submitted from this workspace. Upload the YAML
through the fork's web interface and use the App inclusion template.

Runtime behavior was not tested on an Android device. This recipe is not
byte-identical to the GitHub APK and does not claim reproducible upstream
signatures. F-Droid must sign its resulting APK with its own signing key.
