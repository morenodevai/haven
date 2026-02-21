try {
  const { default: App } = await import("./App.svelte");
  const { mount } = await import("svelte");

  mount(App, {
    target: document.getElementById("app")!,
  });
} catch (e: any) {
  console.error("Haven failed to start:", e);

  // Show a visible error screen instead of blank white
  const root = document.getElementById("app")!;
  root.innerHTML = `
    <div style="display:flex;align-items:center;justify-content:center;height:100%;background:#1e1f22;color:#dcddde;font-family:system-ui,sans-serif;">
      <div style="text-align:center;max-width:420px;padding:40px;">
        <h1 style="font-size:24px;margin-bottom:8px;">Haven failed to start</h1>
        <p style="color:#949ba4;margin-bottom:16px;font-size:14px;">${e?.message || e}</p>
        <button id="haven-retry" style="background:#5865f2;color:white;border:none;border-radius:8px;padding:12px 24px;font-weight:600;cursor:pointer;font-size:14px;">Retry</button>
      </div>
    </div>`;
  document.getElementById("haven-retry")?.addEventListener("click", () => location.reload());

  // Still try to run the auto-updater even if the app crashed
  try {
    const { check } = await import("@tauri-apps/plugin-updater");
    const { relaunch } = await import("@tauri-apps/plugin-process");
    const update = await check();
    if (update) {
      root.innerHTML = `
        <div style="display:flex;align-items:center;justify-content:center;height:100%;background:#1e1f22;color:#dcddde;font-family:system-ui,sans-serif;">
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
