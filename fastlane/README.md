# F-Droid listing metadata

The Android listing for `de.duecki.calibraw` lives in
`metadata/android/en-US/`. F-Droid reads this directory directly; installing
Fastlane or configuring a publishing lane is unnecessary.

Follow the [F-Droid metadata documentation](https://f-droid.org/docs/All_About_Descriptions_Graphics_and_Screenshots/)
when updating the listing:

- `title.txt`: app name, at most 50 characters.
- `short_description.txt`: summary, at most 80 characters.
- `full_description.txt`: full description, at most 4,000 characters; simple HTML is supported.
- `images/icon.png`: existing CalibRaw icon from `packaging/icons/calibraw-1024.png`.
- `images/featureGraphic.jpg`: landscape composite of the three Android screenshots.
- `images/phoneScreenshots/`: Android screenshots in filename order: library, editing, and subject masking. Use PNG or JPEG images; desktop screenshots do not belong in this Android listing.
- `changelogs/default.txt`: current release notes, at most 500 characters. Update this for each release. F-Droid uses it as a fallback when there is no changelog named for the current Android version code.

For release-specific notes, add `changelogs/<versionCode>.txt`, using the actual
Android `versionCode`, not the semantic version from Cargo. The release workflow
currently supplies `1000000 + git rev-list --count HEAD`; other builds can override
it with `-PcalibrawVersionCode`.

Add translations in sibling locale directories under `metadata/android/`;
`en-US` is F-Droid's fallback locale.

F-Droid imports metadata from the latest release it knows about. Include listing
changes in a release to make them available there. If F-Droid's app metadata YAML
already sets `Summary` or `Description`, ask the F-Droid maintainers to remove
those overrides so the repository descriptions can take effect.
