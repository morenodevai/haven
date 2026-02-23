mod commands;

use commands::{crypto, transfer};
use tauri::Manager;

#[cfg(target_os = "windows")]
fn grant_media_permissions(window: &tauri::WebviewWindow) {
    use webview2_com::PermissionRequestedEventHandler;
    use webview2_com::Microsoft::Web::WebView2::Win32::{
        COREWEBVIEW2_PERMISSION_KIND_CAMERA,
        COREWEBVIEW2_PERMISSION_KIND_MICROPHONE,
        COREWEBVIEW2_PERMISSION_STATE_ALLOW,
        COREWEBVIEW2_PERMISSION_STATE_DEFAULT,
    };

    let _ = window.with_webview(|webview| unsafe {
        let core = webview.controller().CoreWebView2().unwrap();
        let mut token = std::mem::zeroed();
        core.add_PermissionRequested(
            &PermissionRequestedEventHandler::create(Box::new(|_, args| {
                if let Some(args) = args {
                    let mut kind = std::mem::zeroed();
                    args.PermissionKind(&mut kind)?;
                    if kind == COREWEBVIEW2_PERMISSION_KIND_MICROPHONE
                        || kind == COREWEBVIEW2_PERMISSION_KIND_CAMERA
                    {
                        args.SetState(COREWEBVIEW2_PERMISSION_STATE_ALLOW)?;
                    } else {
                        args.SetState(COREWEBVIEW2_PERMISSION_STATE_DEFAULT)?;
                    }
                }
                Ok(())
            })),
            &mut token,
        )
        .unwrap();
    });
}

/// Kill any orphaned WebView2 processes from a previous Haven session,
/// clear stale localStorage/session data, but preserve the GPU cache
/// so video rendering works.
fn clean_webview2_data() {
    let local_appdata = match std::env::var("LOCALAPPDATA") {
        Ok(v) if !v.is_empty() => v,
        _ => return,
    };

    let haven_data = std::path::Path::new(&local_appdata).join("com.haven.voice");
    let ebwebview = haven_data.join("EBWebView");

    if !ebwebview.exists() {
        return;
    }

    // Try to remove lock files first (works if no orphan process holds them).
    let lock_held = std::fs::remove_file(ebwebview.join("lockfile")).is_err()
        && ebwebview.join("lockfile").exists();

    if lock_held {
        // Lock file exists but we couldn't delete it → orphan process.
        // Kill all msedgewebview2 processes that belong to OUR data dir.
        {
            use std::os::windows::process::CommandExt;
            let _ = std::process::Command::new("taskkill")
                .args(["/F", "/IM", "msedgewebview2.exe"])
                .creation_flags(0x08000000) // CREATE_NO_WINDOW
                .output();
        }

        // Give the OS a moment to release file handles
        std::thread::sleep(std::time::Duration::from_millis(500));

        // Retry lockfile removal after killing orphans
        let _ = std::fs::remove_file(ebwebview.join("lockfile"));
    }

    // Clear localStorage/session storage so stale auth tokens don't block login,
    // but keep GPU cache (GPUCache, ShaderCache) intact for video rendering.
    let default_profile = ebwebview.join("Default");
    if default_profile.exists() {
        let _ = std::fs::remove_dir_all(default_profile.join("Local Storage"));
        let _ = std::fs::remove_dir_all(default_profile.join("Session Storage"));
        let _ = std::fs::remove_dir_all(default_profile.join("IndexedDB"));
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // Clean WebView2 data BEFORE anything else.
    clean_webview2_data();

    // Ensure the Roaming AppData directory exists so the FS plugin can write
    // the Remember Me credentials file (haven-credentials.json) there.
    if let Ok(appdata) = std::env::var("APPDATA") {
        let dir = std::path::Path::new(&appdata).join("com.haven.voice");
        let _ = std::fs::create_dir_all(&dir);
    }

    // WebView2 flags: autoplay audio, expose real IPs for WebRTC, and ensure
    // GPU hardware acceleration is available for video rendering.
    unsafe {
        std::env::set_var(
            "WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS",
            "--autoplay-policy=no-user-gesture-required \
             --disable-features=WebRtcHideLocalIpsWithMdns \
             --ignore-gpu-blocklist \
             --enable-gpu-rasterization",
        );
    }

    tauri::Builder::default()
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_process::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_fs::init())
        .manage(crypto::KeyStore::default())
        .manage(transfer::TransferEngine::default())
        .invoke_handler(tauri::generate_handler![
            crypto::generate_key,
            crypto::encrypt,
            crypto::decrypt,
            crypto::export_key,
            crypto::import_key,
            transfer::transfer_connect,
            transfer::transfer_send_file,
            transfer::transfer_receive_file,
            transfer::transfer_cancel,
        ])
        .setup(|app| {
            if let Some(window) = app.get_webview_window("main") {
                #[cfg(debug_assertions)]
                window.open_devtools();

                #[cfg(target_os = "windows")]
                grant_media_permissions(&window);
            }
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running Haven");
}
