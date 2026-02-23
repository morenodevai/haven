import App from "./App.svelte";
import { mount } from "svelte";
import { getCurrentWebviewWindow } from "@tauri-apps/api/webviewWindow";

const HAVEN_VERSION = "0.10.10";

// Detect stale cache: if stored version doesn't match, wipe caches and reload.
// This prevents the blank-screen-of-death after updates.
const storedVersion = localStorage.getItem("haven_app_version");
if (storedVersion && storedVersion !== HAVEN_VERSION) {
  // Preserve auth token so user doesn't need to re-login
  const token = localStorage.getItem("haven_token");
  const userId = localStorage.getItem("haven_user_id");
  const username = localStorage.getItem("haven_username");

  sessionStorage.clear();
  // Clear all caches (Service Worker, HTTP, etc.)
  caches?.keys().then((names) => names.forEach((n) => caches.delete(n)));

  // Clear localStorage except auth
  const keysToKeep = new Map<string, string | null>();
  if (token) keysToKeep.set("haven_token", token);
  if (userId) keysToKeep.set("haven_user_id", userId);
  if (username) keysToKeep.set("haven_username", username);
  localStorage.clear();
  keysToKeep.forEach((v, k) => { if (v) localStorage.setItem(k, v); });
  localStorage.setItem("haven_app_version", HAVEN_VERSION);
  location.reload();
} else {
  localStorage.setItem("haven_app_version", HAVEN_VERSION);
}

// Ctrl+Shift+Delete: emergency cache/data clear (escape hatch for blank screens)
document.addEventListener("keydown", (e) => {
  if (e.ctrlKey && e.shiftKey && e.key === "Delete") {
    localStorage.clear();
    sessionStorage.clear();
    caches?.keys().then((names) => names.forEach((n) => caches.delete(n)));
    location.reload();
  }
});

(async () => {
  const appWindow = getCurrentWebviewWindow();

  // Watchdog: if the app doesn't render within 8 seconds, show recovery UI.
  // Catches silent failures where mount() never throws but nothing paints.
  const watchdog = setTimeout(async () => {
    const root = document.getElementById("app");
    if (root && root.children.length === 0) {
      await appWindow.show().catch(() => {});
      root.innerHTML = `
        <div style="display:flex;align-items:center;justify-content:center;height:100%;background:#1a1a2e;color:#e4e4e7;font-family:system-ui,sans-serif;">
          <div style="text-align:center;max-width:420px;padding:40px;">
            <h1 style="font-size:24px;margin-bottom:8px;">Haven is not responding</h1>
            <p style="color:#949ba4;margin-bottom:16px;font-size:14px;">The app may have stale cached data.</p>
            <button id="haven-clear" style="background:#6c63ff;color:white;border:none;border-radius:8px;padding:12px 24px;font-weight:600;cursor:pointer;font-size:14px;margin-right:8px;">Clear data &amp; retry</button>
            <button id="haven-reload" style="background:#333;color:white;border:none;border-radius:8px;padding:12px 24px;font-weight:600;cursor:pointer;font-size:14px;">Retry</button>
          </div>
        </div>`;
      document.getElementById("haven-clear")?.addEventListener("click", () => {
        localStorage.clear();
        sessionStorage.clear();
        caches?.keys().then((names) => names.forEach((n) => caches.delete(n)));
        location.reload();
      });
      document.getElementById("haven-reload")?.addEventListener("click", () => location.reload());
    }
  }, 8000);

  try {
    mount(App, {
      target: document.getElementById("app")!,
    });

    clearTimeout(watchdog);

    // App rendered — now safe to show the window.
    await appWindow.show();
    await appWindow.setFocus();
  } catch (e: any) {
    clearTimeout(watchdog);
    console.error("Haven failed to start:", e);

    // Show window even on crash so the user sees the error screen
    await appWindow.show().catch(() => {});

    // Show a visible error screen instead of blank white
    const root = document.getElementById("app")!;
    root.innerHTML = `
      <div style="display:flex;align-items:center;justify-content:center;height:100%;background:#1a1a2e;color:#e4e4e7;font-family:system-ui,sans-serif;">
        <div style="text-align:center;max-width:420px;padding:40px;">
          <h1 style="font-size:24px;margin-bottom:8px;">Haven failed to start</h1>
          <p style="color:#949ba4;margin-bottom:16px;font-size:14px;">${e?.message || e}</p>
          <div style="margin-top:16px;">
            <button id="haven-clear" style="background:#6c63ff;color:white;border:none;border-radius:8px;padding:12px 24px;font-weight:600;cursor:pointer;font-size:14px;margin-right:8px;">Clear data &amp; retry</button>
            <button id="haven-retry" style="background:#333;color:white;border:none;border-radius:8px;padding:12px 24px;font-weight:600;cursor:pointer;font-size:14px;">Retry</button>
          </div>
        </div>
      </div>`;
    document.getElementById("haven-clear")?.addEventListener("click", () => {
      localStorage.clear();
      sessionStorage.clear();
      caches?.keys().then((names) => names.forEach((n) => caches.delete(n)));
      location.reload();
    });
    document.getElementById("haven-retry")?.addEventListener("click", () => location.reload());

    // Still try to run the auto-updater even if the app crashed
    try {
      const { check } = await import("@tauri-apps/plugin-updater");
      const { relaunch } = await import("@tauri-apps/plugin-process");
      const update = await check();
      if (update) {
        root.innerHTML = `
          <div style="display:flex;align-items:center;justify-content:center;height:100%;background:#1a1a2e;color:#e4e4e7;font-family:system-ui,sans-serif;">
            <div style="text-align:center;max-width:420px;padding:40px;">
              <h1 style="font-size:24px;margin-bottom:8px;">Updating Haven...</h1>
              <p style="color:#949ba4;font-size:14px;">Installing v${update.version}, please wait.</p>
            </div>
          </div>`;
        await update.downloadAndInstall();
        await relaunch();
      }
    } catch (updateErr) {
      console.error("Auto-update also failed:", updateErr);
    }
  }
})();
