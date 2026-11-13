# Rework plan

Status: proposal, nothing implemented. Written 2026-11-13 against commit
`3138a244` (branch `EffectsRework4`, workspace version 1.4.1).

The rework has four aims:

1. Code that is built the same way everywhere, so a change in one place does
   not regress another.
2. Widget IDs that can never collide.
3. The same picture from the fitted preview, the zoomed preview and the export.
4. Two features: customisable shortcuts and a reworked Settings screen.

Every part below says whether it changes behaviour. Parts that do not must
leave the render-parity goldens, the format fixtures and the screenshot
baseline untouched. AGENTS.md still applies: one concern per change, no mixed
formatting, moves and numerical changes.

## 1. Where the code stands

Measured on the commit above.

| Area | Finding |
|---|---|
| Size | 180,645 lines of Rust in 567 files under `crates/`, 18,998 lines of WGSL in 78 files. `calibraw-ui` holds 81,516 of them: `app/` 31,802, `ui/` 40,658. |
| Checks | `fmt`, `clippy`, `test` (render parity included), `arch-check`, `jni-contract`, `ui-lint`, `migrations` and `deny` pass. `cargo xtask cfg-ratchet` fails: `crates/calibraw-ai/src/config.rs` has 7 `target_os` conditions against a baseline of 5. CI runs this check, so CI is red. |
| Check list | AGENTS.md lists six commands to run before finishing. CI and DEVELOPMENT.md run nine: `cfg-ratchet`, `jni-contract` and `ui-lint` are missing from AGENTS.md, which is how the failure above went unnoticed. |
| Widget IDs | IDs are derived from display text and list position. Moduwu's `Slider` (`slider.rs:250`, `:269`) and `AngleDial` (`angle_dial.rs:79`) scope by the translated label. CalibRaw scopes cards by title (`ui/sidebar/adjustment_cards.rs:570`), effect cards by `effect.label()` (`ui/sidebar/mask_effects/mod.rs:174`), effect rows by index (`ui/sidebar/masks/properties.rs:202`, `:259`). Pending card actions and "reveal this card" requests are keyed by the same titles (`adjustment_cards.rs:268`, `:272`). There are 38 `Id::new` literals spread over the UI with no registry. |
| ID checks | egui 0.35 detects a clash only by painting a red marker in debug builds (`context.rs:1097`). Nothing in the checks fails on one. The screenshot harness covers 11 states, none with several masks, components or effects, where clashes arise. |
| Document state in egui memory | The mask rename and delete dialogs and the mask clipboards (a whole `LocalMask` or `MaskComponent`) live in egui's temporary memory under global IDs (`ui/sidebar/masks/menus.rs`). They are not cleared when another photo opens. |
| Render setup | 37 call sites in 21 files build `GpuParams` and 21 build a `RawGpuPipeline`. Each chooses by hand whether to apply card switches, the vignette geometry, the LUT, the profile colour, the light quality and the mask rectangle. Card switches alone are applied by hand in six files, with 13 calls (`preview_visibility.rs`, `saved_edits.rs`, `replay.rs`, `document_load.rs`, `export.rs`, `batch.rs`). |
| Render checks | Render parity (81 cases, 69 of them on `scene`) renders only through the export path. Nothing compares a preview with an export, and one test (`rain_without_depth_does_not_depend_on_the_tile_size`) checks tile independence for one effect. |
| Errors as strings | Three callers decide what to do by searching an error's text for `"GPU pipelines already reserve"` (`interactive_preview.rs:756`, `ai/source.rs:156`, `preview/rebuild.rs:412`). |
| Shortcuts | Handled in seven files with four different egui input calls: `app/edit_history.rs`, `app/sidecar_persistence.rs`, `ui/develop.rs`, `ui/preview/viewport.rs`, `ui/library/keys.rs`, `ui/library/review.rs`, `ui/top_bar.rs`. `ui/shortcuts_dialog.rs` is a hand-kept list of them. Toolbar tooltips spell shortcuts inside translated strings (`ui/top_bar.rs:207`, `:212`, `:245`). `AppAction` has five variants. |
| Views | 168 view functions take `&mut CalibRawApp`. The same operation is implemented in several views: adding an effect is written three times (`masks/properties.rs:227`, `navigation/mobile.rs:348`, `masks/adjustments.rs:69`). Views start threads (`ui/develop.rs:395`, 12 spawns under `ui/library/`) and one builds GPU parameters (`ui/preview/tools/point_color.rs:96`). |
| Settings model | `PerformanceSettings` has about 60 fields of every kind. Saving copies each field by hand from eight state structs into a new value with `..Default::default()` (`app/lifecycle/settings.rs:352`), so a field left out is silently reset. It is called from 58 places, writes on the UI thread, and a failed save is only logged. |
| Settings screen | One scrolling page of 14 cards in one 1,421-line file. Onboarding re-implements many of the same controls (`ui/onboarding.rs`). |
| Background jobs | 39 hand-written thread spawns, 37 channels, 51 `Receiver` fields and 20 cancel flags in `calibraw-ui`. Two helpers are shared (`app/worker.rs`). ARCHITECTURE.md describes the rules of 20 job kinds in prose; nothing in the code enforces them. |
| Imports | 277 `use super::*` in production code (146 in `calibraw-ui`, 76 core, 38 gpu, 10 ai, 7 ffi) against the explicit-imports rule. |
| Error visibility | 110 `log::warn!` and `log::error!` calls in `app/` and `ui/`. The rule is that every failure the user should know of goes through `report_error`. |
| Migrations | 47 markers: 6 due at v1.5.0, 2 at v1.7.0, 39 at v2.0.0. 27 of them sit outside `migrations/` and `legacy/`. `LocalMask` still carries the pre-1.0 fields `effect` and `effect_settings`, and `required_export_tile_halo` has a branch for them. |
| Docs | `docs/DEVELOPMENT.md:131` and `ROADMAP.md:34` link to files in `docs/rework/` that do not exist. `finding.md` in the repository root, not tracked by git, is an audit from v1.3.1 with 38 findings; at least one (F-12, `cargo deny`) no longer holds. |

Not verified: I did not run the app, reproduce a regression, or read every
shader. Where a later part depends on something unchecked, it says so.

## 2. Target design

Five shared mechanisms replace the hand-written copies. Each part of section 3
introduces one.

- **Keys.** Every widget ID comes from a stable key: a serialized name, an
  enum, or a runtime key held by the list item. Never a label, a title or an
  index. Global IDs are declared in one file.
- **Render recipe.** One type turns an edit, a photo and a view into render
  parameters. Preview, export, thumbnails, replay, AI sources and the CLI all
  call it, and a renderer cannot be handed an edit whose switched-off cards
  were not applied.
- **Commands.** Every discrete operation (a button, a menu row, a shortcut) is
  one command with a stable name, a context and an enabled check. Shortcuts,
  menus and toolbars trigger the same command.
- **Settings value.** One `Settings` value is the source of truth. Views read
  it, changes go through it, and it saves itself.
- **View contract.** A control that edits a value takes a mutable reference to
  that value and returns a response. Anything else returns a command. No view
  receives the whole application.

## 3. Parts

### Part 0: guard rails

No behaviour change. Small.

1. Make `cfg-ratchet` pass: move the two new `target_os` conditions in
   `calibraw-ai/src/config.rs` to where platform differences are decided, or
   raise the baseline as a reviewed change. They arrived with the Android
   WebGPU work (`3b4fafde`).
2. Add `cfg-ratchet`, `jni-contract` and `ui-lint` to the verification list in
   AGENTS.md.
3. Fix the two dead links. Re-check the 38 entries of `finding.md`, move the
   ones that still hold into the parts below, and delete the file.
4. Copy the render-parity goldens outside the checkout (they are ignored by
   git and a bless rewrites all of them).
5. Record a screenshot baseline with `cargo xtask baseline-run before-rework`
   and the line counts with `cargo xtask loc --json`. The baseline run renders
   off-screen and opens no window.

### Part 1: preview and export comparison

No behaviour change; adds a check. Approved on 2026-11-13. Medium.

Extends `crates/calibraw-gpu/tests/render_parity.rs` with one more test that
renders every case four ways and compares them with the existing block-mean
measure:

| Render | Path | Compared with |
|---|---|---|
| Export | tiled export at full size (as today) | the golden (as today) |
| Fitted preview | whole image in one graph on a proxy at 1/2 and 1/4 edge, preview quality | the export scaled down |
| Zoomed preview | a region at native scale | the same region cut from the export |
| Other tile size | tiled export with a different tile edge | the export |

The zoomed preview and the other tile size must match within the golden
tolerance of two 8-bit levels: they render the same pixels. The fitted preview
gets an allowance per case, kept in `data/render-parity/preview-allowance.json`
with a reason for each entry above the default. The first run records today's
differences as the allowances, so the check passes; from then on a case that
gets worse fails, and Part 6 lowers the entries.

The test prints a table of the largest difference per case. That table is the
work list for Part 6.

### Part 2: stable widget IDs

No behaviour change apart from remembered open and closed states, which reset
once because their IDs change. Medium. Touches Moduwu, so it needs a published
Moduwu commit and a new pin.

1. **Moduwu.** `Slider` and `AngleDial` take an explicit key
   (`id_salt`, as `NumberField` already does). The label stays the default for
   the gallery only.
2. **Parameter keys.** `FloatParamSpec` has a label but no stable name. Add a
   `key` field: the lane name for effect controls, the serialized field name
   for adjustments. `AdjustmentSlider` passes it on.
3. **`ui/ids.rs`.** All global IDs move there as constants or typed
   constructors (`ids::card(AdjustmentGroup)`, `ids::effect_card(list,
   MaskEffect)`, `ids::mask_row(key)`). The 38 literals, the 13
   `make_persistent_id` calls and the panel IDs in `ui/layout.rs` are replaced.
4. **Cards.** Cards, their pending actions and reveal requests are keyed by
   `AdjustmentGroup` or `CardSwitch`, effect cards by the effect's serialized
   name. An effect appears at most once per list today
   (`mask_effects/mod.rs:360`), so list plus effect is unique.
5. **List items.** Masks and mask components have no identity besides their
   index, so deleting or reordering one hands its widget state to its
   neighbour. Give `LocalMask` and `MaskComponent` a runtime key: not
   serialized, assigned when the item is created or loaded, ignored by
   equality and edit fingerprints, kept by undo. This is the one change to the
   core model in this part; the format fixtures must stay byte-identical.
6. **Document state out of egui memory.** The mask rename and delete dialogs
   and the two mask clipboards move to `MaskState` and are cleared with the
   document. Drag and hover state of single widgets stays in egui memory.
7. **Clash check.** The screenshot harness fails when any frame contains
   egui's clash marker. egui offers no other signal, so the check reads the
   frame's shapes for the marker text; this depends on an egui internal and
   needs re-checking on each egui update. Add harness states where clashes
   occur: three masks with two components and two effects each, Relight with
   two lights, every card open, the library with a selection, each dialog.
   Run the harness in CI on the software adapter, as render parity already is.
8. **Lint.** Two `ui-lint` rules: no ID built from a `label`, `title`, `name`
   or `tr(...)` argument, and no `Id::new("…")` outside `ui/ids.rs`. The
   baseline starts empty.

### Part 3: commands and keymap

No behaviour change: every shortcut does what it does today. Medium to large.

1. **`app/commands.rs`.** One `Command` enum that takes over `AppAction`,
   `GridKey`, `PhotoKey`, `ReviewKey`, `KeyboardZoom` and the keys handled
   inline in `ui/develop.rs` and `ui/top_bar.rs`. Each command has a stable
   name (`edit.undo`, `library.rate.3`), a translated label, a context and an
   `enabled` check.
2. **Contexts.** Global, Library grid, Library search, Develop, and the canvas
   tools (Crop, Masks, Retouch, colour picker). A focused text field and an
   open dialog come first. They replace the three `*_shortcuts_allowed`
   predicates in `app/actions.rs`.
3. **`app/keymap.rs`.** A table of default bindings and a map of overrides. A
   binding is a key, whether it is matched by position or by character, and
   an exact modifier set. Rating keys already match by position
   (`review_key` in `ui/library/review.rs`); the keymap keeps that.
4. **One dispatcher.** It reads this frame's key events once, finds the
   command for the innermost active context, checks `enabled` and runs it. The
   seven handlers go. Exact modifier matching removes the ordering workaround
   in `ui/library/keys.rs:48`.
5. **Generated help.** The shortcuts dialog lists the keymap. Toolbar tooltips
   take their shortcut text from it instead of from the translated string.
6. **Left in the widgets.** Arrow stepping inside a focused slider, range or
   pad, Enter and Escape in dialogs, text editing and Tab traversal stay where
   they are in this part.

Verification: a table mapping every row of today's `shortcuts_dialog.rs` to
its command and default binding, then a walk through it in the running app.

### Part 4: shortcut customisation

New feature. Medium. Scope as decided: everything possible, on every platform
with a keyboard.

| Tier | What becomes bindable |
|---|---|
| 1 | A key for every command, including commands without a default today: tabs, sidebar tabs, tools, toggles (sidebar, filmstrip, histogram, mask overlay, original view), rotate and flip, copy and paste in Develop, reset, export. The exact list comes from the toolbars and menus. |
| 2 | Held keys and modifier roles on the canvas: pan while held (Space), bend a path (Alt), snap an angle (Shift), fine or coarse steps (Shift), set the retouch source (primary modifier plus click). |
| 3 | Mouse buttons and the wheel on the canvas and grid: middle-drag pan, right-click source, double-click fit or 100%, wheel zoom, wheel with a modifier. |
| 4 | Gestures inside shared controls: double-click to reset, arrow stepping. These live in Moduwu, so its controls must accept an input map. Last, because it changes the design library's API. |

Kept fixed, because rebinding them breaks platform conventions: text editing
keys, Tab traversal, Enter and Escape in dialogs, Shift-click and
primary-click selection, context menu on right-click, touch gestures.

- **Storage.** `keymap.json` beside the settings, holding only changed
  commands, so new defaults reach users who did not change them. An empty
  list means unbound. A new file, so it needs no migration.
- **Editor.** A Keyboard page in Settings: commands grouped by context,
  search by name, search by pressing a key, capture of the next key press,
  remove, reset one, reset all. A binding that conflicts within overlapping
  contexts names the other command and offers to move it.
- **Android.** The page shows when a hardware keyboard is attached. That
  needs one query through the platform layer and a check on a device, which
  is skipped for now; desktop comes first.
- **iOS.** Not planned.
- **Translations.** New strings in all 16 locale files.

### Part 5: one render recipe

No behaviour change: no golden is blessed. Large. Depends on Part 1.

1. **`AppliedEdit`** in `calibraw-core`: the edit as it renders, with
   switched-off cards at rest and the look resolved. It can only be made by
   `EditState::into_applied` or by one function over the editor's live state.
2. **`PreviewOverlays`**: what only the editor draws (colour-range display,
   hovered preset, LUT and profile, clipping). Passing it is explicit, so an
   export cannot receive it.
3. **`RenderView`**: whole image, tile (origin and full size) or region.
4. **`RenderRecipe`** in `calibraw-gpu`: builds `GpuParams` and pipeline
   options from the three above plus the quality. The `GpuParams`
   constructors and `with_*` modifiers become crate-private.
5. Move the call sites in groups, one change each: export (`export/worker.rs`,
   `streaming.rs`, `crop.rs`), batch, thumbnails (`saved_edits.rs`), replay,
   full-resolution preview, document load, the editor previews (`rebuild.rs`,
   `navigation.rs`, `processing.rs`, `detail.rs`, `state.rs`,
   `interactive_preview.rs`, `interactive_crop.rs`), AI sources, Camera match,
   lens, the CLI and AI denoise. The colour picker's own setup in
   `ui/preview/tools/point_color.rs` moves to an application handler.
6. A typed `BudgetExceeded` error replaces the three string searches.

This establishes ownership and removes duplication. Verification after each
group: render parity without a bless, the Part 1 table unchanged, AI parity
for the denoise group, the screenshot baseline at tolerance 0.

### Part 6: consistent processing

Changes output. Each fix is its own change with a re-bless and your review in
the app. Size unknown until Part 1 produces its table.

Stages fall into four classes, and each is judged differently:

| Class | Stages | Requirement |
|---|---|---|
| Independent of resolution | tone, colour, LUT, most effects through `resolution_scale` | fitted preview matches the scaled export |
| Native only | demosaic, highlight reconstruction, noise reduction, capture sharpening, chromatic aberration, grain | the zoomed preview matches the export; the fitted preview may differ and its allowance says why |
| Reads the whole image | tone analysis, haze map, image lights, contours, scene bake, Glow map, Lens Blur map | preview and export derive the same values from different resolutions |
| Depends on the tile | Dehaze is known to | none may; the fourth render of Part 1 finds the rest |

Shaders to look at first, because they sample neighbours without using the
shared scale helpers. These are candidates from a text search, not confirmed
defects: `noise.wgsl` (fixed radii of 1 to 16 pixels), `color_denoise.wgsl`,
`film_finish.wgsl`, `neon.wgsl`, `edge_glow.wgsl`, `glow.wgsl`,
`wet_surfaces.wgsl`, `motion_blur.wgsl`, `pixelate.wgsl`, `fog.wgsl`,
`smoke.wgsl`.

Each fix starts from how darktable and ART handle the stage and is never
tuned on one photo.

### Part 7: settings value

No behaviour change; the settings file keeps its name and keys. Medium.

1. Rename `PerformanceSettings` to `Settings` and group its fields
   (appearance, library, editing, colour and lens, export, preview, AI,
   updates, session). The groups are flattened when saved, so the JSON is the
   same.
2. `CalibRawApp` owns one `Settings`. `PreferencesState` and the copies in
   `AiState`, `LibraryState`, `ExportState` and `DevelopUiState` read from it
   or are removed.
3. Remove the hand-copied snapshot and its 58 call sites. A change marks the
   value; it is saved once at the end of that frame and on exit. A failed
   save goes through `report_error`.
4. The eight settings-version conversions move into one module beside the
   model and stay listed in `migrations/mod.rs`.

Verification: load real settings files with the old and the new build, save,
and compare the JSON.

### Part 8: Settings screen

Redesign and new feature. Needs your choice of layout before it is built.
Medium to large.

| Layout | Description |
|---|---|
| A (recommended) | Categories in a list on the left, the chosen page on the right. On Android the list is the first screen and a page opens from it. A search field filters across pages. |
| B | The single page stays, with a pinned index of sections and a search field. |
| C | A or B shown as an overlay on the current tab, so Settings no longer replaces the photo. Can be combined with either. |

Proposed pages for A, from today's 14 cards: General (language, updates,
tips, Discord, usage), Appearance, Library, Editing (card order, copy and
paste, new-photo preset, Camera match), Colour and lens (camera profiles,
LUTs, lens correction), Export, Preview and performance, AI, Keyboard,
About (legal, diagnostics).

- Each simple setting (about 35 switches and choices) is described once:
  key, label, description, page, control, and on which devices it shows. The
  page, the search and onboarding draw from that description. This serves two
  real callers, since onboarding re-implements these controls today.
- Sections with their own behaviour (AI models, LUT library, camera profile
  folders, card order, the keyboard editor) stay hand-written views.
- New harness states for each page, in every theme and viewport.
- New strings in all 16 locale files.

### Part 9: view contract and ownership

No behaviour change. Large, and spread over the other parts rather than done
in one go.

1. **Contract.** Write the two-part rule of section 2 into ARCHITECTURE.md,
   replacing the rule the code does not follow. Add a ratchet that counts
   `app: &mut CalibRawApp` in `ui/` per file (168 today) and only lets it
   fall.
2. **Order.** Settings (21, rebuilt in Part 8), top bar (9), sidebar
   navigation (20), Develop (12), presets (7), masks (11), library (about 45).
   Operations written more than once become one command.
3. **Library.** `ui/library/` (13,225 lines) holds state, scanning, thumbnail
   workers, the clipboard and merge jobs beside its views. State and jobs
   move to `app/library/`, the egui-free parts to `services/`. This
   establishes ownership: `ui/` then contains only views.
4. **Jobs.** A `Job` handle that owns the receiver, the cancel flag and the
   document generation, cancels when dropped and rejects stale results. Move
   a job kind to it only where it replaces a hand-written copy of those
   rules. This is the riskiest step, as each job has its own contract in
   ARCHITECTURE.md; one job kind per change, checked against that table.
5. **Imports.** Replace `use super::*` file by file in separate mechanical
   changes, with a ratchet like the one for `cfg`.
6. **Errors.** Go through the 110 log-only warnings and errors and route the
   ones a user should see through `report_error`.
7. **Large files.** Split only where one file has several owners:
   `ui/settings.rs` (by page, in Part 8), `masks/properties.rs`,
   `lifecycle/document_load.rs`, `gpu/resources.rs`.

### Part 10: legacy and release

1. Shipping as 1.5.0 retires the six markers due then: two in
   `calibraw-ai/src/runtime/package.rs`, two in
   `calibraw-ui/src/app/ai/models.rs`, one in
   `migrations/desktop_model_folder.rs` and one in
   `app/lifecycle/startup.rs`, with their tests, noted in the release notes.
2. The 39 markers for v2.0.0 stay; the policy in AGENTS.md does not allow
   removing them earlier.
3. Move separable compatibility code into `migrations/`. First candidate: the
   pre-1.0 `effect` and `effect_settings` fields of `LocalMask`, converted on
   decode as `highlight_glow` is, which removes the fields and the branch in
   `required_export_tile_halo`.
4. Release notes for every look change from Part 6.

## 4. Order

Two tracks that do not touch the same code:

- Rendering: Part 0, 1, 5, 6.
- Interface: Part 0, 2, 3, 4, 7, 8.

Part 9 accompanies the interface track from Part 3 on. Part 10 closes the
release.

## 5. Version

Recommended split:

- **1.4.2**: Parts 0, 1, 2 and 5. No visible change apart from fixes; no
  migration is retired.
- **1.5.0**: Parts 3, 4, 6, 7, 8 and 10. New features and look changes, and
  the version at which the six due markers must be deleted anyway.

## 6. Not in this plan

- Features beyond shortcuts and Settings. "And more" was not specified.
- Known regressions. None were named, and reading the code found none beyond
  the failing `cfg-ratchet`.
- iOS.
- Tests on Android devices.

## 7. Open decisions

1. Settings layout: A, B or C (Part 8).
2. Runtime keys on masks and components in the core model (Part 2, step 5).
   Recommended; the alternative keeps index-based IDs for list rows.
3. The release split of section 5.
4. Which regressions you have seen, so each gets a root cause and a part.
5. Which further features belong in 1.5.0.
