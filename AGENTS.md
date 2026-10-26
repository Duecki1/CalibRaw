# CalibRaw agent rules

CalibRaw is a Rust, wgpu/WGSL and egui RAW editor for Linux, Android, Windows and macOS.
Architecture, dependency rules and concurrency contracts: [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md).
Commands, platform setup and UI conventions: [docs/DEVELOPMENT.md](docs/DEVELOPMENT.md).

## Working rules

- Preserve behavior: features, interactions, numerical output and persisted formats (settings, presets, sidecars, edit history). Redesigns, algorithm changes and new features are separate changes.
- Keep each change focused; do not mix broad formatting, file moves, API changes and numerical changes.
- An extraction must remove duplication, serve a real additional caller, or establish ownership. Say which.
- Delegate focused implementation and exploration tasks; keep architecture, ambiguous requirements, integration and final verification with the coordinator.

## Rust

- Respect crate ownership and the dependency rules in docs/ARCHITECTURE.md; `cargo xtask arch-check` enforces them.
- Keep APIs narrow and fields private where invariants matter. Model exclusive states with enums, and units or coordinate spaces with types where confusion causes bugs.
- Borrow by default; justify clones of image buffers and edit snapshots. Use `Arc` only for data genuinely shared across jobs.
- Use typed errors where callers distinguish failures and add context at boundaries. No production `unwrap`/`expect` without a stated invariant.
- Keep imports explicit in production modules. Document units, ownership and numerical contracts; explain unusual choices, not obvious code.
- State the safety assumptions of every `unsafe` block. Wrap JNI references, file descriptors and GPU reservations in RAII types.

## Migrations

- Code that reads sidecars, presets or settings saved in an older layout lives in `crates/calibraw-core/src/migrations/`, one file per layout change, and runs when the file is decoded, so the current model keeps no legacy fields or branches. Compatibility code that cannot be separated stays next to the code it adapts and is listed in `migrations/mod.rs`.
- Mark every migration with a comment `migration: remove in vX.Y.Z`, the first version allowed to drop it. User files (sidecars, presets, settings): the next major version after the release that ships it. App-data cleanup: two minor versions later. Deleting a migration deletes its tests and old-layout fixtures and is noted in the release notes.
- Report a migrated sidecar through `LoadedSidecar::migrated`, so it is saved again in the current layout.

## WGSL and GPU

- Production shaders live in `.wgsl` files. Document colour spaces, coordinate systems, boundary handling and precision.
- Rust/WGSL buffer layouts, bindings and shared IDs are verified by `layout_contract_tests`; change both sides and the test together.
- Structural cleanup preserves pass order, clamps, negative/HDR handling, tile halos and cache behavior.

## UI

- Reusable presentation (tokens, controls, frames) belongs to Moduwu; photo data, image canvases and domain controls stay in CalibRaw.
- Views take narrow inputs and return actions. Application handlers own mutation, persistence and background work.
- Shared controls keep role, label, value, focus, disabled state and actions for accessibility; custom-painted controls supply them explicitly.
- Keep serialized names stable and preserve dismissal, focus and navigation behavior ([UI conventions](docs/DEVELOPMENT.md#ui-conventions)).
- No explanatory text in panels, cards or tool rows. Help goes in a tooltip, and only when a control is not self-explanatory; never on dragged controls (sliders, ranges, dials, handles), whose help goes on a static label or heading. Settings may keep descriptions.
- A requested feature tip includes recording its clip of the real app with the demo recorder; follow [Feature tips](docs/DEVELOPMENT.md#feature-tips).

## Tests

- Do not write unit tests. Behaviour is checked by render parity, format fixtures, the contract checks (`layout_contract_tests`, shader validation, `arch-check`, `ui-lint`), AI parity, the screenshot baselines and use of the app ([Render parity and format fixtures](docs/DEVELOPMENT.md#render-parity-and-format-fixtures)).
- Cover new rendering with a case in `data/render-parity/`, and a new file layout or import mapping (including a migration) with a fixture in `data/formats/`: add the file, bless with `CALIBRAW_PARITY_BLESS=1`, and review the changed goldens. A change meant to alter output re-blesses and says so.
- Write a Rust test only when asked, or for a concurrency, cancellation or cache-invalidation bug. It drives the public API in one test function; bug fixes otherwise ship without a test.
- Never test layout, painting, visibility, labels, wiring, accessors, single parsing rules or single numerical cases. Never add a test harness, fixture builder or test-only API (`#[cfg(test)]` constructors or accessors).
- Code that changes or is removed takes its tests with it; do not keep an old test passing with test-only code.
- While iterating, test only the affected crate (`cargo test -p <crate>`); run the full checks once at the end.

## Verification

Run before finishing a change (commands and Android/GPU checks: [docs/DEVELOPMENT.md](docs/DEVELOPMENT.md#checks)):

```sh
cargo fmt --all -- --check
cargo clippy --locked --workspace --all-targets --all-features -- -D warnings -W clippy::perf -W clippy::large_stack_arrays -W clippy::redundant_clone -W unreachable-pub
cargo test --locked --workspace --all-targets
cargo xtask arch-check
cargo xtask migrations
cargo deny check
```

Before a commit, merge or release, delete every migration that `cargo xtask migrations` reports as due, with its tests. The check fails once the workspace version reaches a marker, so a version bump retires them.
