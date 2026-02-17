import { writable, get } from "svelte/store";
import * as api from "../ipc/api";

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

// Restore session from localStorage on startup
const savedToken = localStorage.getItem("haven_token");
const savedUserId = localStorage.getItem("haven_user_id");
const savedUsername = localStorage.getItem("haven_username");
if (savedToken && savedUserId && savedUsername) {
  auth.set({
    loggedIn: true,
    userId: savedUserId,
    username: savedUsername,
    token: savedToken,
  });
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
}

function saveSession(userId: string, username: string, token: string) {
  localStorage.setItem("haven_token", token);
  localStorage.setItem("haven_user_id", userId);
  localStorage.setItem("haven_username", username);
  auth.set({ loggedIn: true, userId, username, token });
}
