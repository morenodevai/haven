import { writable } from "svelte/store";
import { check } from "@tauri-apps/plugin-updater";
import { relaunch } from "@tauri-apps/plugin-process";

export const updateAvailable = writable(false);
export const updateVersion = writable("");
export const updateProgress = writable<"idle" | "checking" | "downloading" | "installing" | "error">("idle");
export const updateError = writable("");

let cachedUpdate: Awaited<ReturnType<typeof check>> | null = null;

export async function checkForUpdate() {
  try {
    updateProgress.set("checking");
    const token = import.meta.env.VITE_UPDATER_TOKEN;
    const update = await check({
      headers: token ? { Authorization: `token ${token}` } : {},
    });
    if (update) {
      cachedUpdate = update;
      updateVersion.set(update.version);
      updateAvailable.set(true);
    }
  } catch (e) {
    console.error("Update check failed:", e);
  } finally {
    updateProgress.set("idle");
  }
}

export async function installUpdate() {
  if (!cachedUpdate) return;

  try {
    updateProgress.set("downloading");
    await cachedUpdate.downloadAndInstall();
    updateProgress.set("installing");
    await relaunch();
  } catch (e: any) {
    const msg = e?.message || String(e);
    console.error("Update install failed:", msg);
    updateError.set(msg);
    updateProgress.set("error");
  }
}
