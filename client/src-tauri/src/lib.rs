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

/// Kill orphaned WebView2 processes and clean stale locks so WebView2 can start.
///
/// We preserve Local Storage (auth state) and the GPU shader cache, but
/// aggressively remove ALL lock files and expendable caches (Service Worker,
/// Session Storage) that are known to prevent WebView2 from initializing.
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

    // Always kill orphaned WebView2 processes. This runs before Tauri creates
    // its own WebView2, so we won't kill our own instance.
    {
        use std::os::windows::process::CommandExt;
        let _ = std::process::Command::new("taskkill")
            .args(["/F", "/IM", "msedgewebview2.exe"])
            .creation_flags(0x08000000) // CREATE_NO_WINDOW
            .output();
    }

    // Wait for OS to fully release file handles
    std::thread::sleep(std::time::Duration::from_millis(1000));

    // Recursively clean all lock files and expendable caches
    clean_webview2_locks(&ebwebview);
}

/// Remove lock files, temp files, and expendable caches throughout the
/// EBWebView directory tree. Keeps Local Storage and GPU shader cache intact.
fn clean_webview2_locks(dir: &std::path::Path) {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");

        if path.is_dir() {
            // Nuke expendable directories that can get corrupted and block startup
            if name == "Service Worker" || name == "Session Storage" {
                let _ = std::fs::remove_dir_all(&path);
            } else {
                clean_webview2_locks(&path);
            }
        } else {
            // Remove lock files and known problematic temp files
            if name == "lockfile"
                || name == "lock"
                || name == "LOCK"
                || name == "LOG"
                || name == "LOG.old"
                || name.ends_with(".tmp")
            {
                let _ = std::fs::remove_file(&path);
            }
        }
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

                // Safety net: if JS never calls show() (WebView2 failed to
                // initialize), force the window visible after 10 seconds so
                // the user isn't staring at nothing.
                let win = window.clone();
                std::thread::spawn(move || {
                    std::thread::sleep(std::time::Duration::from_secs(10));
                    if !win.is_visible().unwrap_or(true) {
                        let _ = win.show();
                    }
                });
            }
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running Haven");
}
