// Prevents additional console window on Windows in release, DO NOT REMOVE!!
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
mod models;
use models::db;

#[tauri::command]
fn main() {
    db::DataBase::connect().unwrap();
    my_app_lib::run()
}
