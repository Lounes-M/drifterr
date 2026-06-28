// Prevents a second console window on Windows in release.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    drifterr_desktop_lib::run()
}
