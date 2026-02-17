mod commands;

use commands::crypto;
use tauri::Manager;

#[cfg(target_os = "windows")]
fn grant_media_permissions(window: &tauri::WebviewWindow) {
    use webview2_com::PermissionRequestedEventHandler;
    use webview2_com::Microsoft::Web::WebView2::Win32::{
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
                    if kind == COREWEBVIEW2_PERMISSION_KIND_MICROPHONE {
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
            }
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running Haven");
}
