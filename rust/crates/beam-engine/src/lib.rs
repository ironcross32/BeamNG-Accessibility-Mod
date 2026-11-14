//! The BEAM engine: everything in beamtel.py that is not the window. Does not depend on Tauri.
//!
//! The module map, owners and the shared-state rules are in `rust_plans/engine-map.md`.

/// Placeholder body for a function a wave-2 workstream has not ported yet. Logs once per
/// call at warn level and evaluates to nothing; the stub then returns a neutral value.
#[macro_export]
macro_rules! not_ported {
    ($name:expr) => {
        log::warn!("not yet ported: {}", $name)
    };
}

pub mod ai_describer;
pub mod ai_layer;
pub mod browsers;
pub mod capture;
pub mod compat;
pub mod console;
pub mod engine;
pub mod f9_commands;
pub mod keyboard;
pub mod listeners;
pub mod mcp_server;
pub mod menus;
pub mod mod_install;
pub mod net;
pub mod speech_out;
pub mod state;
pub mod telemetry;
pub mod timer;
pub mod ui_bridge;
pub mod ui_sink;
pub mod units;
pub mod updater;
pub mod vehicle_spawner;
pub mod virtual_browser;
pub mod win_util;
