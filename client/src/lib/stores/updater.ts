import { writable } from "svelte/store";
import { check } from "@tauri-apps/plugin-updater";
import { relaunch } from "@tauri-apps/plugin-process";

export const updateAvailable = writable(false);
export const updateVersion = writable("");
export const updateProgress = writable<"idle" | "checking" | "downloading" | "installing">("idle");

let cachedUpdate: Awaited<ReturnType<typeof check>> | null = null;

export async function checkForUpdate() {
  try {
    updateProgress.set("checking");
    const update = await check();
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
  } catch (e) {
    console.error("Update install failed:", e);
    updateProgress.set("idle");
  }
}
