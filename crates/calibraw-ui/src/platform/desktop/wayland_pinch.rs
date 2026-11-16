//! Touchpad pinch on Wayland. winit 0.30 does not bind
//! `zwp_pointer_gestures_v1`, so the compositor's pinch never reaches egui.
//! This binds it on the window's own connection, with an event queue of its
//! own read by a background thread, and hands each pinch step to the next
//! frame as `egui::Event::Zoom`, as winit does on macOS.
//!
//! Reading the shared socket from this thread does not starve winit:
//! libwayland lets the last of the threads that prepared a read do it, so
//! while winit sleeps prepared on the socket it is woken and reads itself;
//! events this thread reads for winit's queue while winit is busy make
//! winit's next `prepare_read` fail and dispatch them before it sleeps.

use eframe::egui;
use raw_window_handle::{HasDisplayHandle, RawDisplayHandle};
use std::os::fd::AsRawFd;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread::JoinHandle;
use wayland_backend::client::Backend;
use wayland_client::globals::{registry_queue_init, GlobalListContents};
use wayland_client::protocol::wl_pointer::{self, WlPointer};
use wayland_client::protocol::wl_registry::{self, WlRegistry};
use wayland_client::protocol::wl_seat::{self, WlSeat};
use wayland_client::{Connection, Dispatch, EventQueue, Proxy, QueueHandle, WEnum};
use wayland_protocols::wp::pointer_gestures::zv1::client::zwp_pointer_gesture_pinch_v1::{
    self, ZwpPointerGesturePinchV1,
};
use wayland_protocols::wp::pointer_gestures::zv1::client::zwp_pointer_gestures_v1::{
    self, ZwpPointerGesturesV1,
};

/// How long the thread waits on the socket before checking `stop`, in ms.
const POLL_TIMEOUT_MS: libc::c_int = 100;

/// Zoom steps received since the last frame, shared with the thread.
type PendingEvents = Arc<Mutex<Vec<egui::Event>>>;

/// The listener thread; dropping it stops the thread and waits for it.
pub(super) struct WaylandPinch {
    events: PendingEvents,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl WaylandPinch {
    /// Starts listening, or `None` off Wayland or when the compositor has no
    /// pointer gestures.
    pub(super) fn start(cc: &eframe::CreationContext<'_>) -> Option<Self> {
        let RawDisplayHandle::Wayland(handle) = cc.display_handle().ok()?.as_raw() else {
            return None;
        };
        // SAFETY: the pointer is the live `wl_display` of winit's connection.
        // The connection outlives this listener: eframe keeps the event loop
        // that owns it in a thread-local until the main thread ends, while
        // this listener is dropped with the app (or by `detach_ui`) first.
        // A foreign backend never closes the display itself.
        let backend = unsafe { Backend::from_foreign_display(handle.display.as_ptr().cast()) };
        let connection = Connection::from_backend(backend);
        let (globals, mut queue) = match registry_queue_init::<PinchState>(&connection) {
            Ok(init) => init,
            Err(error) => {
                log::warn!("Touchpad pinch unavailable: {error}");
                return None;
            }
        };
        let handle = queue.handle();
        let Ok(gestures) = globals.bind::<ZwpPointerGesturesV1, _, _>(&handle, 1..=3, ()) else {
            log::info!("Touchpad pinch unavailable: the compositor has no pointer gestures");
            return None;
        };
        let Ok(seat) = globals.bind::<WlSeat, _, _>(&handle, 1..=5, ()) else {
            return None;
        };
        let events = PendingEvents::default();
        let mut state = PinchState {
            gestures,
            seat,
            pinch: None,
            scale: 1.0,
            events: Arc::clone(&events),
            ctx: cc.egui_ctx.clone(),
        };
        // Delivers the seat's capabilities, which create the pinch.
        if let Err(error) = queue.roundtrip(&mut state) {
            log::warn!("Touchpad pinch unavailable: {error}");
            return None;
        }
        let stop = Arc::new(AtomicBool::new(false));
        let thread_stop = Arc::clone(&stop);
        let thread = std::thread::Builder::new()
            .name("calibraw-wayland-pinch".to_owned())
            .spawn(move || listen(queue, state, &thread_stop));
        match thread {
            Ok(thread) => Some(Self {
                events,
                stop,
                thread: Some(thread),
            }),
            Err(error) => {
                log::warn!("Touchpad pinch unavailable: {error}");
                None
            }
        }
    }

    /// Moves the zoom steps received since the last call into `events`.
    pub(super) fn take_events(&self, events: &mut Vec<egui::Event>) {
        events.append(&mut self.events.lock().unwrap_or_else(PoisonError::into_inner));
    }
}

impl Drop for WaylandPinch {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// Dispatches the queue until `stop` is set, then releases the objects.
fn listen(mut queue: EventQueue<PinchState>, mut state: PinchState, stop: &AtomicBool) {
    while !stop.load(Ordering::Relaxed) {
        if let Err(error) = queue.dispatch_pending(&mut state) {
            log::warn!("Touchpad pinch stopped: {error}");
            break;
        }
        if let Err(error) = queue.flush() {
            log::warn!("Touchpad pinch stopped: {error}");
            break;
        }
        // `None` means events are already queued: dispatch them first.
        let Some(guard) = queue.prepare_read() else {
            continue;
        };
        let mut poll_fd = libc::pollfd {
            fd: guard.connection_fd().as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        };
        // SAFETY: `poll_fd` is one valid `pollfd` for the duration of the
        // call, and the descriptor stays open while `guard` holds the read.
        let ready = unsafe { libc::poll(&mut poll_fd, 1, POLL_TIMEOUT_MS) };
        if ready > 0 {
            if let Err(wayland_client::backend::WaylandError::Io(error)) = guard.read() {
                // Another reader taking the events first is expected.
                if error.kind() != std::io::ErrorKind::WouldBlock {
                    log::warn!("Touchpad pinch stopped: {error}");
                    break;
                }
            }
        }
        // Dropping an unused guard cancels the read.
    }
    state.release();
    let _ = queue.flush();
}

struct PinchState {
    gestures: ZwpPointerGesturesV1,
    seat: WlSeat,
    /// A pointer of our own and its pinch, while the seat has a pointer.
    pinch: Option<(WlPointer, ZwpPointerGesturePinchV1)>,
    /// The pinch's scale at the previous update, relative to its start.
    scale: f64,
    events: PendingEvents,
    ctx: egui::Context,
}

impl PinchState {
    fn drop_pinch(&mut self) {
        if let Some((pointer, pinch)) = self.pinch.take() {
            pinch.destroy();
            if pointer.version() >= 3 {
                pointer.release();
            }
        }
    }

    fn release(&mut self) {
        self.drop_pinch();
        if self.gestures.version() >= 2 {
            self.gestures.release();
        }
        if self.seat.version() >= 5 {
            self.seat.release();
        }
    }

    /// Queues a zoom step for the next frame, folded into a step not yet
    /// taken, and wakes the UI.
    fn push_zoom(&self, factor: f32) {
        let mut events = self.events.lock().unwrap_or_else(PoisonError::into_inner);
        match events.last_mut() {
            Some(egui::Event::Zoom(pending)) => *pending *= factor,
            _ => events.push(egui::Event::Zoom(factor)),
        }
        drop(events);
        self.ctx.request_repaint();
    }
}

impl Dispatch<WlRegistry, GlobalListContents> for PinchState {
    fn event(
        _: &mut Self,
        _: &WlRegistry,
        _: wl_registry::Event,
        _: &GlobalListContents,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        // Globals added later (a second seat) are not followed.
    }
}

impl Dispatch<WlSeat, ()> for PinchState {
    fn event(
        state: &mut Self,
        seat: &WlSeat,
        event: wl_seat::Event,
        _: &(),
        _: &Connection,
        handle: &QueueHandle<Self>,
    ) {
        let wl_seat::Event::Capabilities {
            capabilities: WEnum::Value(capabilities),
        } = event
        else {
            return;
        };
        let has_pointer = capabilities.contains(wl_seat::Capability::Pointer);
        if has_pointer && state.pinch.is_none() {
            let pointer = seat.get_pointer(handle, ());
            let pinch = state.gestures.get_pinch_gesture(&pointer, handle, ());
            state.pinch = Some((pointer, pinch));
        } else if !has_pointer {
            state.drop_pinch();
        }
    }
}

impl Dispatch<WlPointer, ()> for PinchState {
    fn event(
        _: &mut Self,
        _: &WlPointer,
        _: wl_pointer::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        // winit's own pointer handles motion and buttons.
    }
}

impl Dispatch<ZwpPointerGesturesV1, ()> for PinchState {
    fn event(
        _: &mut Self,
        _: &ZwpPointerGesturesV1,
        _: zwp_pointer_gestures_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<ZwpPointerGesturePinchV1, ()> for PinchState {
    fn event(
        state: &mut Self,
        _: &ZwpPointerGesturePinchV1,
        event: zwp_pointer_gesture_pinch_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            zwp_pointer_gesture_pinch_v1::Event::Begin { .. } => state.scale = 1.0,
            zwp_pointer_gesture_pinch_v1::Event::Update { scale, .. } => {
                // `scale` is relative to the start of the pinch.
                if scale > 0.0 && state.scale > 0.0 {
                    let factor = (scale / state.scale) as f32;
                    if (factor - 1.0).abs() > f32::EPSILON {
                        state.push_zoom(factor);
                    }
                }
                state.scale = scale;
            }
            zwp_pointer_gesture_pinch_v1::Event::End { .. } => state.scale = 1.0,
            _ => {}
        }
    }
}
