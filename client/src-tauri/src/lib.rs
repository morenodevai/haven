mod commands;

use commands::crypto;

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
        .run(tauri::generate_context!())
        .expect("error while running Haven");
}
