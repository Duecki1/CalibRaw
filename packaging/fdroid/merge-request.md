Adds CalibRaw, a GPL-3.0-or-later RAW photo editor with GPU rendering,
non-destructive editing and optional on-device AI tools. Builds the ARM64
Android app from release `v1.2.1`, commit
`<full hash of v1.2.1>`.

Related packaging request: https://gitlab.com/fdroid/rfp/-/work_items/4487

## Checklist

I am the app author. The fork is public; please confirm the uploaded source branch is unprotected before submitting.

### Policy

- [x] The app complies with the [inclusion criteria](https://f-droid.org/docs/Inclusion_Policy).
- [x] The original app author has been notified (and does not oppose the inclusion). If you are not the author, please paste the link of the reply from the author.
- [x] The upstream app source code repo contains the app metadata in a [Fastlane](https://gitlab.com/snippets/1895688) or [Triple-T](https://gitlab.com/snippets/1901490) folder structure. The summary and description must be included and images, icon, and changelog should also be provided for better user experience. The `en-US` locale must be included.

### Docs

- [x] Please read [the guide](https://gitlab.com/fdroid/fdroiddata/-/blob/master/CONTRIBUTING.md) first if this is your first contribution.
- [x] Please make sure your metadata follows the best practice in [our templates](https://gitlab.com/fdroid/fdroiddata/tree/master/templates).
- [x] Please read the [Build Metadata Reference](https://f-droid.org/docs/Build_Metadata_Reference/) and make sure your metadata is valid.
- [x] Please read the [Quick Start Guide](https://f-droid.org/en/docs/Submitting_to_F-Droid_Quick_Start_Guide/).

### Merge Request Setup

- [x] The title of this merge request should follow "New app: app name" format.
- [ ] Please make sure your fdroiddata fork is public and your branch is not protected. See <https://docs.gitlab.com/user/project/repository/branches/protected/>.
- [ ] Please read [our Git guide](https://gitlab.com/fdroid/wiki/-/wikis/Tips-for-fdroiddata-contributors/Git-Usage) if you don't know how to rebase your branch. Don't rebase your branch if there is no conflict.
- [x] All related [fdroiddata](https://gitlab.com/fdroid/fdroiddata/issues) and [RFP issues](https://gitlab.com/fdroid/rfp/issues) have been referenced in this merge request
- [x] Please only submit one app in one MR.

### Metadata

- [x] Metadata must be put in `metadata/<applicationId>.yml`.
- [x] Metadata must be a valid YAML file.
- [x] Metadata must use LF as line ending.
- [x] Don't add summary/description/changelog/images or anything that should be provided in upstream repo. Please check the Changes tab to make sure there is no other unrelated files added in the MR.
- [x] Releases are tagged; exception for initial static updates: `AutoUpdateMode: None` and `UpdateCheckMode: Static` until a new release tag contains `fdroid_version_code` (see below).
- [x] There is an issue tracker and contact info of the author so that we can report bugs and contact the author.
- [x] An AuthorName must be added. It doesn't need to be the real name.
- [x] External repos are added as git submodules instead of srclibs. No srclibs are used. Native dependencies are fetched from immutable, hash-verified upstream source archives by the upstream CMake build.
- [ ] Enable [Reproducible Builds](https://f-droid.org/docs/Reproducible_Builds). We'll use your signature for improved security/reliability, also allowing users to switch between different channels. Do note that if you don't enable reproducible build then the apk will be signed with our key so you can't enable it later. If you can't enable this, please add the reasons here.
- [x] Setup abi split if the APK is large and the splitted ones can be much smaller. Only ARM64 is supported upstream; there is no multi-ABI build to split.
- [x] Only the latest versions should be kept in the metadata before it's merged. If you update the metadata, please replace the old versions with the new ones.
- [x] Don't add any disabled versions in the metadata.
- [x] The `commit` field should be the full hash. Please don't use tag or branch in commit.

### Pipeline

- [ ] All pipelines should pass.
- [ ] All warnings and errors in the Reports tab should be fixed or explained.
- [x] F-Droid CI runners are under GitLab's FOSS program, so there's no need for you to pay for any CI time. If Gitlab starts asking for phone numbers or credit cards don't submit anything, just leave a note in the MR so we know we need to trigger the CI.

## Packaging notes

The recipe builds the pinned `v1.2.1` commit and uses `AutoUpdateMode: Version`
with `UpdateCheckMode: Tags` for stable `vMAJOR.MINOR.PATCH` tags. `v1.2.0` is
the first tag containing both `fdroid_version_code` in `Cargo.toml` and the
Fastlane listing, so no listing import is needed. Older tags lack the field and
are skipped. An earlier revision of this recipe built an untagged commit after
`v1.2.0` under version `1.2.0`; it now builds the `v1.2.1` tag, which includes
that code (including the R8 change) under a new version name and code. The exact YAML and `fdroid rewritemeta` / `fdroid checkupdates
--auto` steps are in upstream `packaging/fdroid/README.md`.

LibRaw, Lensfun and the native support libraries build from pinned source.
ONNX Runtime uses Microsoft's official MIT-licensed Android AAR from Maven
Central, pinned by SHA-256; its only packaged component is
`libonnxruntime.so`. The ort crate's own binary downloader is disabled.
Optional AI model weights are not bundled; their licenses and provenance are
documented in upstream `THIRD_PARTY_NOTICES.md`, and downloads require consent.

Upstream-signature reproducible builds are not enabled because GitHub releases
statically link ONNX Runtime, while this recipe links the Maven Central shared
runtime. F-Droid will sign the resulting APK. Only ARM64 is supported upstream,
so there is no multi-ABI APK to split.

Prior local validation: `fdroid lint` and a full local `fdroid build` passed,
with source scanning and APK scanning enabled. The resulting APK passed the
16 KB ELF and ZIP alignment checks. F-Droid's `check_tags` routine detected
simulated future release tags and selected the highest committed Android
version code; those tests did not cover the published tags missing that field.

The prior local build used the developer workstation's toolchains. The latest
[GitLab pipeline](https://gitlab.com/dueei12/uw-u-data-calib-raw/-/pipelines/2910143849)
passed its build. The corrected recipe preserves all build commands and now
passes local `fdroid lint`, schema validation and `fdroid checkupdates --auto`.
Canonical `fdroid rewritemeta` output is stable on a second formatting pass;
the local formatter also reproduces the failed job's formatting artifact
exactly. CI on the replacement metadata remains to run.

Replace `metadata/de.duecki.calibraw.yml` on the source branch of
[!51073](https://gitlab.com/fdroid/fdroiddata/-/merge_requests/51073) with the
fixed, formatted local YAML, commit to that branch and rerun CI. The pipeline
checklist remains open pending those results. Inclusion review is still
pending; runtime behavior has not been tested on an Android device.
