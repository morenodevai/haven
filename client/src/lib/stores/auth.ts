import { writable, get } from "svelte/store";
import * as api from "../ipc/api";

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
    // Now login to get the full response with username
    const loginRes = await api.login(username, password);
    saveSession(loginRes.user_id, username, loginRes.token);
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
  auth.set(initial);
}

function saveSession(userId: string, username: string, token: string) {
  localStorage.setItem("haven_token", token);
  localStorage.setItem("haven_user_id", userId);
  localStorage.setItem("haven_username", username);
  auth.set({ loggedIn: true, userId, username, token });
}
