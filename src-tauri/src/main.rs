// No console window in release builds; debug builds keep one for log output.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    beam_app::run();
}
