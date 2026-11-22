//! Road guidance, the driving assistant and their recorders.

pub mod assistant_diagnostics;
pub mod assistant_route;
pub mod challenge_results;
pub mod driving_assistant;
pub mod road_diagnostics;
pub mod road_guidance;

/// pathlib and Windows text-mode file behaviour the recorders depend on.
pub mod pyfs;
/// Python's `json` encoder/decoder and the builtins applied to decoded JSON.
pub mod pyjson;
/// Python's `time.monotonic`, `time.time` and the `datetime` forms the recorders use.
pub mod pytime;
