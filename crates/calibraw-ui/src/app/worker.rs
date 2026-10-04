#[cfg(not(target_os = "android"))]
use eframe::egui;
use std::sync::mpsc;

#[cfg(not(target_os = "android"))]
pub(crate) fn spawn_ui_worker<T, F>(context: &egui::Context, work: F) -> mpsc::Receiver<T>
where
    T: Send + 'static,
    F: FnOnce() -> T + Send + 'static,
{
    let (sender, receiver) = mpsc::channel();
    let context = context.clone();
    std::thread::spawn(move || {
        let result = work();
        let _ = sender.send(result);
        context.request_repaint();
    });
    receiver
}

pub(crate) fn drain_worker_events<T>(
    receiver: Option<&mpsc::Receiver<T>>,
    is_terminal: impl Fn(&T) -> bool,
) -> (Vec<T>, bool) {
    let mut events = Vec::new();
    let mut disconnected = false;
    let Some(receiver) = receiver else {
        return (events, disconnected);
    };
    loop {
        match receiver.try_recv() {
            Ok(event) => {
                let terminal = is_terminal(&event);
                events.push(event);
                if terminal {
                    break;
                }
            }
            Err(mpsc::TryRecvError::Empty) => break,
            Err(mpsc::TryRecvError::Disconnected) => {
                disconnected = true;
                break;
            }
        }
    }
    (events, disconnected)
}

#[cfg(test)]
mod tests {
    use super::drain_worker_events;
    use std::sync::mpsc;

    #[test]
    fn draining_stops_after_a_terminal_event_and_keeps_later_ones() {
        let (sender, receiver) = mpsc::channel();
        for event in [1, 2, 99, 3] {
            sender.send(event).unwrap();
        }
        let (events, disconnected) = drain_worker_events(Some(&receiver), |event| *event == 99);
        assert_eq!(events, [1, 2, 99]);
        assert!(!disconnected);
        assert_eq!(receiver.try_recv(), Ok(3));
    }

    #[test]
    fn a_dead_worker_is_reported_after_its_queued_events() {
        let (sender, receiver) = mpsc::channel();
        sender.send(1).unwrap();
        drop(sender);
        let (events, disconnected) = drain_worker_events(Some(&receiver), |_| false);
        assert_eq!(events, [1]);
        assert!(disconnected);

        // A worker that sent its terminal event before exiting is not reported.
        let (sender, receiver) = mpsc::channel();
        sender.send(99).unwrap();
        drop(sender);
        let (events, disconnected) = drain_worker_events(Some(&receiver), |event| *event == 99);
        assert_eq!(events, [99]);
        assert!(!disconnected);
    }

    #[test]
    fn no_receiver_means_no_events() {
        let (events, disconnected) = drain_worker_events::<u8>(None, |_| true);
        assert!(events.is_empty());
        assert!(!disconnected);
    }
}
