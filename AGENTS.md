# CalibRaw agent rules

CalibRaw is a Rust, wgpu/WGSL and egui RAW editor for Linux, Android, Windows and macOS.
Architecture, dependency rules and concurrency contracts: [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md).
Commands, platform setup and UI conventions: [docs/DEVELOPMENT.md](docs/DEVELOPMENT.md).

## Working rules

- Preserve behavior: features, interactions, numerical output and persisted formats (settings, presets, sidecars, edit history). Redesigns, algorithm changes and new features are separate changes.
- Keep each change focused; do not mix broad formatting, file moves, API changes and numerical changes.
- An extraction must remove duplication, serve a real additional caller, or establish ownership. Say which.
- Delegate focused implementation, exploration and test tasks; keep architecture, ambiguous requirements, integration and final verification with the coordinator.

## Rust

- Respect crate ownership and the dependency rules in docs/ARCHITECTURE.md; `cargo xtask arch-check` enforces them.
- Keep APIs narrow and fields private where invariants matter. Model exclusive states with enums, and units or coordinate spaces with types where confusion causes bugs.
- Borrow by default; justify clones of image buffers and edit snapshots. Use `Arc` only for data genuinely shared across jobs.
- Use typed errors where callers distinguish failures and add context at boundaries. No production `unwrap`/`expect` without a stated invariant.
- Keep imports explicit in production modules. Document units, ownership and numerical contracts; explain unusual choices, not obvious code.
- State the safety assumptions of every `unsafe` block. Wrap JNI references, file descriptors and GPU reservations in RAII types.

## WGSL and GPU

- Production shaders live in `.wgsl` files. Document colour spaces, coordinate systems, boundary handling and precision.
- Rust/WGSL buffer layouts, bindings and shared IDs are verified by `layout_contract_tests`; change both sides and the test together.
- Structural cleanup preserves pass order, clamps, negative/HDR handling, tile halos and cache behavior.

## UI

- Reusable presentation (tokens, controls, frames) belongs to Moduwu; photo data, image canvases and domain controls stay in CalibRaw.
- Views take narrow inputs and return actions. Application handlers own mutation, persistence and background work.
- Shared controls keep role, label, value, focus, disabled state and actions for accessibility; custom-painted controls supply them explicitly.
- Keep serialized names stable and preserve dismissal, focus and navigation behavior ([UI conventions](docs/DEVELOPMENT.md#ui-conventions)).

## Verification

Run before finishing a change (commands and Android/GPU checks: [docs/DEVELOPMENT.md](docs/DEVELOPMENT.md#checks)):

```sh
cargo fmt --all -- --check
cargo clippy --locked --workspace --all-targets --all-features -- -D warnings -W clippy::perf -W clippy::large_stack_arrays -W clippy::redundant_clone -W unreachable-pub
cargo test --locked --workspace --all-targets
cargo xtask arch-check
cargo deny check
```
