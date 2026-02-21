mod commands;

use commands::crypto;
use tauri::Manager;

#[cfg(target_os = "windows")]
fn grant_media_permissions(window: &tauri::WebviewWindow) {
    use webview2_com::PermissionRequestedEventHandler;
    use webview2_com::Microsoft::Web::WebView2::Win32::{
        COREWEBVIEW2_PERMISSION_KIND_CAMERA,
        COREWEBVIEW2_PERMISSION_KIND_MICROPHONE,
        COREWEBVIEW2_PERMISSION_STATE_ALLOW,
        COREWEBVIEW2_PERMISSION_STATE_DENY,
    };

    let _ = window.with_webview(|webview| unsafe {
        let core = webview.controller().CoreWebView2().unwrap();
        let mut token = std::mem::zeroed();
        core.add_PermissionRequested(
            &PermissionRequestedEventHandler::create(Box::new(|_, args| {
                if let Some(args) = args {
                    let mut kind = std::mem::zeroed();
                    args.PermissionKind(&mut kind)?;
                    // Allow both microphone and camera for voice + video chat
                    if kind == COREWEBVIEW2_PERMISSION_KIND_MICROPHONE
                        || kind == COREWEBVIEW2_PERMISSION_KIND_CAMERA
                    {
                        args.SetState(COREWEBVIEW2_PERMISSION_STATE_ALLOW)?;
                    } else {
                        args.SetState(COREWEBVIEW2_PERMISSION_STATE_DENY)?;
                    }
                }
                Ok(())
            })),
            &mut token,
        )
        .unwrap();
    });
}

/// Force WebView2 to repaint by doing a tiny resize bounce.
/// Works around a known WebView2 bug where the initial render sometimes
/// produces a blank white screen until something triggers a
/// layout/repaint (e.g. opening DevTools, resizing the window).
#[cfg(target_os = "windows")]
fn force_repaint(window: &tauri::WebviewWindow) {
    use tauri::PhysicalSize;

    let win = window.clone();
    std::thread::spawn(move || {
        // Small delay to let WebView2 finish its first layout pass
        std::thread::sleep(std::time::Duration::from_millis(150));
        if let Ok(size) = win.inner_size() {
            let _ = win.set_size(PhysicalSize::new(size.width + 1, size.height));
            std::thread::sleep(std::time::Duration::from_millis(50));
            let _ = win.set_size(size);
        }
    });
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // WebView2: allow autoplay audio (for voice chat) and expose real local IPs
    // for WebRTC ICE candidates (mDNS obfuscation breaks LAN connectivity).
    // SAFETY: called before any threads are spawned.
    unsafe {
        std::env::set_var(
            "WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS",
            "--autoplay-policy=no-user-gesture-required --disable-features=WebRtcHideLocalIpsWithMdns",
        );
    }

    tauri::Builder::default()
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_process::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_fs::init())
        .manage(crypto::KeyStore::default())
        .invoke_handler(tauri::generate_handler![
            crypto::generate_key,
            crypto::encrypt,
            crypto::decrypt,
            crypto::export_key,
            crypto::import_key,
        ])
        .setup(|app| {
            if let Some(window) = app.get_webview_window("main") {
                #[cfg(debug_assertions)]
                window.open_devtools();

                #[cfg(target_os = "windows")]
                grant_media_permissions(&window);

                #[cfg(target_os = "windows")]
                force_repaint(&window);
            }
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running Haven");
}
