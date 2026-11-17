# CalibRaw's winit patch

Source: the published `winit` 0.30.13 crate, with its Apache-2.0 license retained.
All upstream source is unchanged except:

- `src/platform_impl/android/mod.rs`: exit on `MainEvent::Destroy`, and release
  the process-wide event-loop creation guard when the Android loop drops.
- `src/event_loop.rs`: allow the guard-reset helper on Android as well as Web.

These are the Destroy/recreation fixes proposed in upstream
[PR 4739](https://github.com/rust-windowing/winit/pull/4739) and
[PR 4752](https://github.com/rust-windowing/winit/pull/4752), backported to the
pinned release without their unrelated changes. Remove this patch when a
compatible upstream release includes both fixes.

CalibRaw's Android entry point sets `NativeOptions::run_and_return = false`:
eframe's default caches the loop in thread-local storage, which otherwise drops
only after android-activity has notified Java that the native thread stopped.
That lets the next Activity race the old loop's guard reset. The owned loop
instead drops before `android_main` returns and before that notification.

No desktop, numerical, shader or input behavior is changed by this patch.
