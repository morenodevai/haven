const DEFAULT_BASE_URL = "http://72.49.142.48:3210";
let baseUrl = localStorage.getItem("haven_base_url") ?? DEFAULT_BASE_URL;

export function setBaseUrl(url: string) {
  baseUrl = url;
  localStorage.setItem("haven_base_url", url);
}

export function getBaseUrl(): string {
  return baseUrl;
}

async function request<T>(
  path: string,
  options: RequestInit = {}
): Promise<T> {
  const token = localStorage.getItem("haven_token");

  const headers: Record<string, string> = {
    "Content-Type": "application/json",
    ...(options.headers as Record<string, string>),
  };

  if (token) {
    headers["Authorization"] = `Bearer ${token}`;
  }

  const res = await fetch(`${baseUrl}${path}`, {
    ...options,
    headers,
  });

  if (!res.ok) {
    const text = await res.text();
    throw new Error(`${res.status}: ${text}`);
  }

  return res.json();
}

// -- Auth --

export interface RegisterResponse {
  user_id: string;
  token: string;
}

export interface LoginResponse {
  user_id: string;
  username: string;
  token: string;
}

export async function register(
  username: string,
  password: string
): Promise<RegisterResponse> {
  return request("/auth/register", {
    method: "POST",
    body: JSON.stringify({ username, password }),
  });
}

export async function login(
  username: string,
  password: string
): Promise<LoginResponse> {
  return request("/auth/login", {
    method: "POST",
    body: JSON.stringify({ username, password }),
  });
}

// -- Messages --

export interface ReactionGroup {
  emoji: string;
  count: number;
  user_ids: string[];
}

export interface MessageResponse {
  id: string;
  channel_id: string;
  author_id: string;
  author_username: string;
  ciphertext: string;
  nonce: string;
  created_at: string;
  reactions: ReactionGroup[];
}

export async function sendMessage(
  channelId: string,
  ciphertext: string,
  nonce: string
): Promise<MessageResponse> {
  return request(`/channels/${channelId}/messages`, {
    method: "POST",
    body: JSON.stringify({ ciphertext, nonce }),
  });
}

export async function getMessages(
  channelId: string,
  limit: number = 50
): Promise<MessageResponse[]> {
  return request(`/channels/${channelId}/messages?limit=${limit}`);
}

// -- Reactions --

export async function toggleReaction(
  channelId: string,
  messageId: string,
  emoji: string
): Promise<{ added: boolean }> {
  return request(`/channels/${channelId}/messages/${messageId}/reactions`, {
    method: "POST",
    body: JSON.stringify({ emoji }),
  });
}
