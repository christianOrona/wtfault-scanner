//! Windows binary entry point.
//!
//! `windows_subsystem = "windows"` in release keeps a console window from
//! appearing behind the app; debug builds keep the console so `tracing` output
//! is visible while working on it.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
#![warn(missing_docs)]

fn main() {
    ai_mechanic_desktop_lib::run();
}
