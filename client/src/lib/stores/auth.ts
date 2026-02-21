import { writable, get } from "svelte/store";
import * as api from "../ipc/api";
import { exists, readTextFile, writeTextFile, remove, BaseDirectory } from "@tauri-apps/plugin-fs";
import { setChannelKey } from "./messages";

// SECURITY NOTE: The JWT token is currently stored in localStorage, which is
// accessible to any JavaScript running in the page context (XSS risk).
// The Content-Security-Policy headers mitigate this, but do not eliminate it.
//
// Migration path (TODO):
//   1. Use tauri-plugin-store for encrypted on-disk storage, or
//   2. Use the OS keychain via tauri-plugin-keychain for true credential isolation.
//
// Until then, tokens are short-lived (24h) and cleared on logout to limit exposure.

interface AuthState {
  loggedIn: boolean;
  userId: string | null;
  username: string | null;
  token: string | null;
}

const initial: AuthState = {
  loggedIn: false,
  userId: null,
  username: null,
  token: null,
};

export const auth = writable<AuthState>(initial);
export const authError = writable<string | null>(null);

// "Remember Me" state — true if the user checked the box on their last login
export const rememberMe = writable<boolean>(false);

// Whether auto-login from saved credentials is in progress
export const autoLoginInProgress = writable<boolean>(false);
export const autoLoginSkipped = writable<boolean>(false);

// --- Persistent credentials (Remember Me) ---
// Stored in {AppData}/haven-credentials.json which survives NSIS app updates.
// Contains username + password for re-authentication, plus the channel key.

const CREDENTIALS_FILE = "haven-credentials.json";

interface SavedCredentials {
  username: string;
  password: string;
  channelKey: string | null;
}

/** Save credentials to AppData for Remember Me. */
export async function saveRememberMe(
  username: string,
  password: string,
  channelKey: string | null,
): Promise<void> {
  try {
    const data: SavedCredentials = { username, password, channelKey };
    await writeTextFile(CREDENTIALS_FILE, JSON.stringify(data), {
      baseDir: BaseDirectory.AppData,
    });
  } catch (e) {
    console.warn("[auth] Failed to save Remember Me credentials:", e);
  }
}

/** Clear saved credentials from AppData. */
export async function clearRememberMe(): Promise<void> {
  try {
    const fileExists = await exists(CREDENTIALS_FILE, {
      baseDir: BaseDirectory.AppData,
    });
    if (fileExists) {
      await remove(CREDENTIALS_FILE, { baseDir: BaseDirectory.AppData });
    }
  } catch (e) {
    console.warn("[auth] Failed to clear Remember Me credentials:", e);
  }
}

/** Load saved credentials from AppData (returns null if none exist). */
export async function loadRememberMe(): Promise<SavedCredentials | null> {
  try {
    const fileExists = await exists(CREDENTIALS_FILE, {
      baseDir: BaseDirectory.AppData,
    });
    if (!fileExists) return null;

    const text = await readTextFile(CREDENTIALS_FILE, {
      baseDir: BaseDirectory.AppData,
    });
    const data = JSON.parse(text) as SavedCredentials;
    if (data.username && data.password) return data;
    return null;
  } catch (e) {
    console.warn("[auth] Failed to load Remember Me credentials:", e);
    return null;
  }
}

/** Attempt auto-login from saved credentials. Called on app startup. */
export async function tryAutoLogin(): Promise<boolean> {
  if (get(autoLoginSkipped)) return false;

  const saved = await loadRememberMe();
  if (!saved) return false;

  autoLoginInProgress.set(true);
  rememberMe.set(true);

  try {
    // Check if user skipped during the async login call
    if (get(autoLoginSkipped)) return false;

    const res = await api.login(saved.username, saved.password);

    if (get(autoLoginSkipped)) return false;

    saveSession(res.user_id, res.username, res.token);

    // Restore channel key if saved (set both localStorage and the reactive store)
    if (saved.channelKey) {
      setChannelKey(saved.channelKey);
    }

    return true;
  } catch (e: any) {
    console.warn("[auth] Auto-login failed:", e);
    // Saved credentials are invalid — clear them
    await clearRememberMe();
    rememberMe.set(false);
    return false;
  } finally {
    autoLoginInProgress.set(false);
  }
}

/** Skip auto-login and show manual login form. */
export function skipAutoLogin() {
  autoLoginSkipped.set(true);
  autoLoginInProgress.set(false);
}

// Restore session from localStorage on startup
const savedToken = localStorage.getItem("haven_token");
const savedUserId = localStorage.getItem("haven_user_id");
const savedUsername = localStorage.getItem("haven_username");
if (savedToken && savedUserId && savedUsername) {
  // Check if token is expired before restoring session
  const exp = getTokenExp(savedToken);
  if (exp && exp * 1000 > Date.now()) {
    auth.set({
      loggedIn: true,
      userId: savedUserId,
      username: savedUsername,
      token: savedToken,
    });
    scheduleTokenRefresh(savedToken);
  } else {
    // Token expired — clear and force re-login
    localStorage.removeItem("haven_token");
    localStorage.removeItem("haven_user_id");
    localStorage.removeItem("haven_username");
    localStorage.removeItem("haven_channel_key");
  }
}

export async function register(username: string, password: string) {
  authError.set(null);
  try {
    const res = await api.register(username, password);
    // Use the token from RegisterResponse directly — no need to call login,
    // which would trigger a redundant Argon2 hash on the server.
    saveSession(res.user_id, username, res.token);
  } catch (e: any) {
    authError.set(e.message.includes("409") ? "Username already taken" : e.message);
    throw e;
  }
}

export async function login(username: string, password: string) {
  authError.set(null);
  try {
    const res = await api.login(username, password);
    saveSession(res.user_id, res.username, res.token);
  } catch (e: any) {
    authError.set(e.message.includes("401") ? "Invalid username or password" : e.message);
    throw e;
  }
}

export function logout() {
  localStorage.removeItem("haven_token");
  localStorage.removeItem("haven_user_id");
  localStorage.removeItem("haven_username");
  // Also clear the channel encryption key on logout to limit exposure window
  localStorage.removeItem("haven_channel_key");
  auth.set(initial);
  // Clear saved credentials on explicit logout
  clearRememberMe().catch(() => {});
  rememberMe.set(false);
}

function saveSession(userId: string, username: string, token: string) {
  localStorage.setItem("haven_token", token);
  localStorage.setItem("haven_user_id", userId);
  localStorage.setItem("haven_username", username);
  auth.set({ loggedIn: true, userId, username, token });
  scheduleTokenRefresh(token);
}

// Decode JWT payload to get expiry
function getTokenExp(token: string): number | null {
  try {
    const payload = JSON.parse(atob(token.split(".")[1]));
    return payload.exp ?? null;
  } catch {
    return null;
  }
}

let refreshTimer: number | null = null;

function scheduleTokenRefresh(token: string) {
  if (refreshTimer) clearTimeout(refreshTimer);

  const exp = getTokenExp(token);
  if (!exp) return;

  // Refresh 1 hour before expiry
  const msUntilRefresh = (exp * 1000) - Date.now() - (60 * 60 * 1000);

  if (msUntilRefresh <= 0) {
    // Token expires within the hour — refresh now
    doRefresh();
  } else {
    refreshTimer = window.setTimeout(doRefresh, msUntilRefresh);
  }
}

async function doRefresh() {
  for (let attempt = 0; attempt < 3; attempt++) {
    try {
      const { token } = await api.refreshToken();
      const userId = localStorage.getItem("haven_user_id");
      const username = localStorage.getItem("haven_username");
      if (userId && username) {
        saveSession(userId, username, token);
      }
      return;
    } catch (e) {
      console.warn(`Token refresh attempt ${attempt + 1} failed:`, e);
      if (attempt < 2) {
        await new Promise((r) => setTimeout(r, 1000 * Math.pow(2, attempt)));
      }
    }
  }
  // All retries exhausted — force re-login
  logout();
}
