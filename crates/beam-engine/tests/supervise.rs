//! `timer::supervise` (python-bugs.md 3-9 and the other listener killers): a listener
//! that panics or returns while the stop event is clear runs again; once the stop event
//! is set it is left stopped.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use beam_engine::timer::{Event, supervise};

#[test]
fn restarts_after_a_panic_and_an_unexpected_return_then_stops() {
    let prev = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {}));
    let stop = Arc::new(Event::new());
    let runs = Arc::new(AtomicUsize::new(0));
    let (s, r) = (Arc::clone(&stop), Arc::clone(&runs));
    let started = std::time::Instant::now();
    supervise("test listener", &stop, &move || {
        match r.fetch_add(1, Ordering::SeqCst) {
            0 => panic!("a malformed packet"),
            1 => {}       // a socket error ended the loop
            _ => s.set(), // the engine is closing
        }
    });
    std::panic::set_hook(prev);
    assert_eq!(runs.load(Ordering::SeqCst), 3);
    // Backoff 1 s after the panic, then 2 s after the return.
    let waited = started.elapsed().as_secs_f64();
    assert!((2.9..6.0).contains(&waited), "waited {waited} s");
}

#[test]
fn a_set_stop_event_is_never_restarted() {
    let stop = Event::new();
    stop.set();
    let runs = AtomicUsize::new(0);
    supervise("test listener", &stop, &|| {
        runs.fetch_add(1, Ordering::SeqCst);
    });
    assert_eq!(runs.load(Ordering::SeqCst), 1);
}
