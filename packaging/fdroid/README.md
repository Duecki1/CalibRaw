# F-Droid submission and updates

The submission recipe is [de.duecki.calibraw.yml](de.duecki.calibraw.yml).
It builds the existing `v1.1.1` release, commit
`60cc0005cc6cc0479bb2c8db8a7b963554f7a393`, with Android version code
`1000707`. This is a source build, not an upload of the GitHub release APK.

## Submit using GitLab's website

1. Open your [fdroiddata fork](https://gitlab.com/dueei12/uw-u-data-calib-raw).
2. Create a branch named `de.duecki.calibraw` from `master`.
3. Open the `metadata` directory on that branch and upload
   `de.duecki.calibraw.yml` from this directory. Its final repository path must
   be **`metadata/de.duecki.calibraw.yml`**.
4. Commit with the message `New app: CalibRaw`.
5. Open a merge request from that branch to **`fdroid/fdroiddata`**, target
   branch **`master`**, with title `New app: CalibRaw`.
6. Use [merge-request.md](merge-request.md) as the description. Check the
   GitLab pipelines and address any reviewer comments before merging.

Only the YAML recipe goes in fdroiddata. Keep this README and the
`Cargo.toml` change in CalibRaw. No terminal push or GitLab access token is
needed for this workflow.

The `metadata/de.duecki.calibraw/` directory would hold listing assets and
translations, not the build YAML. CalibRaw already provides its listing under
  `fastlane/metadata/android/en-US/`, so those assets are not copied into the fork.

The `v1.1.1` tag predates the listing. The recipe imports only its `fastlane`
directory from the pinned upstream commit
`7bde63b1861bf40e00e1009fed1cc8f51bcd6c49` for this initial build. Future tags
containing `fastlane` use the listing supplied by that release.

## Release updates

Before your next release, commit the new `fdroid_version_code` field from
CalibRaw's `Cargo.toml` to the upstream CalibRaw repository. Existing release
tags do not contain it; the initial build is pinned explicitly in the recipe.
Automatic update detection starts with the first new release tag containing
this field.

For each subsequent release:

1. Bump `[workspace.package].version` in `Cargo.toml`, for example to `1.1.2`,
   and update the workspace package entries in `Cargo.lock` as usual.
2. Increase `[workspace.metadata].fdroid_version_code`, for example from
   `1000707` to `1000708`. This must increase even when a patch number resets
   during a minor or major version bump.
3. Add release notes to
   `fastlane/metadata/android/en-US/changelogs/1000708.txt` (maximum 500
   characters), using the F-Droid version code. Continue updating `default.txt`
   for the existing GitHub release workflow.
4. Commit and push those changes, then tag that commit `v1.1.2` and push the
   tag. The tag version must match the workspace package version. Only tag
   stable versions matching `vMAJOR.MINOR.PATCH` for this update stream.

F-Droid's updater reads the committed version code and version name from
`Cargo.toml`, selects a newer matching tag, and generates the next build entry.
Updates still go through F-Droid's build and publication process, so they do
not appear immediately. You do not need another packaging request for ordinary
releases. If the NDK, Rust, SDK, CMake, ONNX Runtime, or build procedure changes,
submit a merge request adjusting the fdroiddata recipe.

F-Droid passes its version code to Gradle with `calibrawVersionCode`.
The existing GitHub release workflow keeps its Git-history-based version
codes; it is not changed by this submission.

## Native dependencies and signing

The recipe builds LibRaw, Lensfun and their native dependencies from pinned
source. ONNX Runtime comes from Microsoft's official MIT-licensed
`com.microsoft.onnxruntime:onnxruntime-android:1.24.2` AAR on Maven Central,
verified against a pinned SHA-256. It uses only `libonnxruntime.so`; the Rust
crate's binary downloader is disabled. No AI model weights are bundled.

F-Droid's scanner removes Gradle wrappers, and F-Droid removes upstream signing
configuration. The recipe therefore uses F-Droid's Gradle and builds Rust
directly rather than invoking the upstream wrapper-based `xtask` release path.

The resulting APK uses a shared ONNX Runtime, while the GitHub APK uses a
static runtime. They are not byte-identical, so upstream-signature
reproducible-build verification is not enabled. F-Droid will sign this APK with
its own key. Switching between the GitHub and F-Droid builds requires
uninstalling and reinstalling; export or back up app data first.

Keep the existing application ID, `de.duecki.calibraw`. F-Droid recommends
using a domain you own but does not require proof of domain ownership.
Changing the existing ID to `de.dueckis.calibraw` would create a separate Android
app and break update continuity for current installations.

## References

- [F-Droid submission guide](https://f-droid.org/en/docs/Submitting_to_F-Droid_Quick_Start_Guide/)
- [Build metadata reference](https://f-droid.org/en/docs/Build_Metadata_Reference/)
- [Inclusion policy](https://f-droid.org/en/docs/Inclusion_Policy/)
- [Existing packaging request, #4487](https://gitlab.com/fdroid/rfp/-/work_items/4487)
- [Android application ID guidance](https://developer.android.com/build/configure-app-module#application-id)
