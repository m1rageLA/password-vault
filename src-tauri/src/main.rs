// Prevents additional console window on Windows in release, DO NOT REMOVE!!
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
mod models;

#[tauri::command]
fn main() {
    my_app_lib::run();
}
