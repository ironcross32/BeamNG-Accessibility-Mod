//! Owner: E8. `vehicle_spawner.py`: the accessible vehicle spawner (F11 modal), placement
//! editor, its own UDP pair (4460/4461) and key handling.
//!
//! `init` takes what `_run_engine` injects (beamtel.py:12578-12591). STUB: signatures fixed
//! by EA; bodies to be ported by E8 (who may turn this file into a directory module).

use std::sync::Arc;

use indexmap::IndexMap;

use crate::not_ported;
use crate::state::SlotInfo;

/// `say_fn(text, interrupt=True, exclude_from_buffer=False, source="python")`.
pub type SayFn = Arc<dyn Fn(&str, bool, bool, &str) + Send + Sync>;

/// The arguments of `vehicle_spawner.init(...)`.
#[derive(Clone)]
pub struct InitArgs {
    pub say_fn: SayFn,
    pub is_focused_fn: Arc<dyn Fn() -> bool + Send + Sync>,
    pub get_slots_fn: Arc<dyn Fn() -> IndexMap<i64, SlotInfo> + Send + Sync>,
    pub close_others_fn: Arc<dyn Fn() + Send + Sync>,
    /// `audio_controller.trigger_placement_ping(rung, azimuth_deg, pitch_mult)`.
    pub ping_fn: Arc<dyn Fn(i64, f64, f64) + Send + Sync>,
    pub request_info_fn: Arc<
        dyn Fn(Option<&str>, Option<&str>, f64) -> (Vec<String>, Option<(String, String)>)
            + Send
            + Sync,
    >,
}

/// `init(say_fn, is_focused_fn, logger, get_slots_fn, close_others_fn, ping_fn, request_info_fn)`.
pub fn init(args: InitArgs) {
    let _ = args;
    not_ported!("vehicle_spawner::init");
}

/// `start(stop_event)`: listener + worker threads, the F11 hotkey, the catalog request.
pub fn start() {
    not_ported!("vehicle_spawner::start");
}

/// `is_modal_open()`.
pub fn is_modal_open() -> bool {
    false
}

/// `close_modal()`.
pub fn close_modal() {
    not_ported!("vehicle_spawner::close_modal");
}
