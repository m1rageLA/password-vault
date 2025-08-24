mod models;

use models::db::{DataBase, Entry, EntryListItem, VaultError};
use secrecy::SecretString;
use tauri::State;
use tauri::Manager;

fn err_ui(e: VaultError) -> String {
    match e {
        VaultError::Locked => "Хранилище заблокировано — разблокируй мастер-паролем".into(),
        VaultError::BadMasterPassword => "Неверный мастер-пароль или чужой бэкап".into(),
        VaultError::NotInitialized => "Хранилище не инициализировано".into(),
        other => other.to_string(),
    }
}

#[tauri::command]
fn greet(name: &str) -> String {
    format!("Hello, {}! You've been greeted from Rust!", name)
}

#[tauri::command]
async fn vault_init(db: State<'_, DataBase>, master: String) -> Result<(), String> {
    db.init_master(SecretString::new(master)).await.map_err(err_ui)
}

#[tauri::command]
async fn vault_unlock(db: State<'_, DataBase>, master: String) -> Result<(), String> {
    db.unlock(SecretString::new(master)).await.map_err(err_ui)
}

#[tauri::command]
async fn vault_lock(db: State<'_, DataBase>) -> Result<(), String> {
    db.lock().await;
    Ok(())
}

#[tauri::command]
async fn vault_is_unlocked(db: State<'_, DataBase>) -> Result<bool, String> {
    Ok(db.is_unlocked().await)
}

#[tauri::command]
async fn add_entry(
    db: State<'_, DataBase>,
    site: String,
    username: String,
    password: String,
    notes: Option<String>,
) -> Result<i64, String> {
    db.add_entry(&site, &username, &password, notes.as_deref())
        .await
        .map_err(err_ui)
}

#[tauri::command]
async fn get_entry(db: State<'_, DataBase>, id: i64) -> Result<Entry, String> {
    db.get_entry(id).await.map_err(err_ui)
}

#[tauri::command]
async fn list_entries(
    db: State<'_, DataBase>,
    search: Option<String>,
) -> Result<Vec<EntryListItem>, String> {
    db.list_entries(search.as_deref()).await.map_err(err_ui)
}

#[tauri::command]
async fn update_entry(
    db: State<'_, DataBase>,
    id: i64,
    site: String,
    username: String,
    password: Option<String>,
    notes: Option<String>,
) -> Result<(), String> {
    db.update_entry(id, &site, &username, password.as_deref(), notes.as_deref())
        .await
        .map_err(err_ui)
}

#[tauri::command]
async fn delete_entry(db: State<'_, DataBase>, id: i64) -> Result<(), String> {
    db.delete_entry(id).await.map_err(err_ui)
}

#[tauri::command]
async fn generate_password(
    db: State<'_, DataBase>,
    length: usize,
    use_digits: bool,
    use_upper: bool,
    use_symbols: bool,
) -> Result<String, String> {
    let s = db.generate_password(length, use_digits, use_upper, use_symbols);
    Ok(s)
}

#[tauri::command]
async fn export_backup(db: State<'_, DataBase>, path: String) -> Result<(), String> {
    db.export_encrypted_backup(path).await.map_err(err_ui)
}

#[tauri::command]
async fn import_backup(db: State<'_, DataBase>, path: String) -> Result<usize, String> {
    db.import_encrypted_backup(path).await.map_err(err_ui)
}

// Совместимость со старым фронтом:
#[tauri::command]
async fn add_password(db: State<'_, DataBase>, password: String) -> Result<i64, String> {
    db.add_entry("example.com", "user", &password, None)
        .await
        .map_err(err_ui)
}

#[tauri::command]
async fn get_password(db: State<'_, DataBase>, id: i64) -> Result<String, String> {
    db.get_password(id).await.map_err(err_ui)
}

#[tauri::command]
async fn export_backup_bytes(db: tauri::State<'_, DataBase>) -> Result<Vec<u8>, String> {
    db.export_encrypted_bytes().await.map_err(err_ui)
}

#[tauri::command]
async fn import_backup_bytes(db: tauri::State<'_, DataBase>, data: Vec<u8>) -> Result<usize, String> {
    db.import_encrypted_bytes(&data).await.map_err(err_ui)
}


#[cfg_attr(mobile, tauri::mobile_entry_point)]
#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init()) // <— Добавить ЭТО
        .setup(|app| {
            let db = tauri::async_runtime::block_on(DataBase::open("app.db"))
                .map_err(|e| anyhow::anyhow!(e))?;
            app.manage(db);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            greet,
            vault_init, vault_unlock, vault_lock, vault_is_unlocked,
            add_entry, get_entry, list_entries, update_entry, delete_entry,
            generate_password,
            export_backup, import_backup, import_backup_bytes, export_backup_bytes,
            add_password, get_password
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

