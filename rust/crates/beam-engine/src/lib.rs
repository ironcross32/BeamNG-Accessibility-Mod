//! The BEAM engine: everything in beamtel.py that is not the window. Does not depend on Tauri.

pub mod ai_describer;
pub mod ai_layer;
pub mod browsers;
pub mod capture;
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
