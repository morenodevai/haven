mod commands;

use commands::crypto;
use tauri::Manager;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![
            crypto::generate_key,
            crypto::encrypt,
            crypto::decrypt,
            crypto::export_key,
            crypto::import_key,
        ])
        .setup(|app| {
            if let Some(window) = app.get_webview_window("main") {
                window.open_devtools();
            }
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running Haven");
}
