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
  options: RequestInit = {},
  timeoutMs = 10000,
): Promise<T> {
  const controller = new AbortController();
  const timeout = setTimeout(() => controller.abort(), timeoutMs);

  try {
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
      signal: controller.signal,
    });

    if (!res.ok) {
      const text = await res.text();
      throw new Error(`${res.status}: ${text}`);
    }

    return res.json();
  } finally {
    clearTimeout(timeout);
  }
}

async function requestWithRetry<T>(
  path: string,
  options: RequestInit = {},
  retries = 2,
): Promise<T> {
  for (let attempt = 0; attempt <= retries; attempt++) {
    try {
      return await request<T>(path, options);
    } catch (e: any) {
      const isRetryable =
        e.name === "AbortError" ||
        (e.message && /^5\d{2}:/.test(e.message)) ||
        e.message?.includes("fetch");
      if (!isRetryable || attempt === retries) throw e;
      await new Promise((r) => setTimeout(r, 500 * Math.pow(2, attempt)));
    }
  }
  throw new Error("unreachable");
}

// -- Token refresh --

export async function refreshToken(): Promise<{ token: string }> {
  return request("/auth/refresh", { method: "POST" });
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
  password: string,
): Promise<RegisterResponse> {
  return request("/auth/register", {
    method: "POST",
    body: JSON.stringify({ username, password }),
  });
}

export async function login(
  username: string,
  password: string,
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
  nonce: string,
): Promise<MessageResponse> {
  return request(`/channels/${channelId}/messages`, {
    method: "POST",
    body: JSON.stringify({ ciphertext, nonce }),
  }, 60000);
}

export async function getMessages(
  channelId: string,
  limit: number = 50,
): Promise<MessageResponse[]> {
  return requestWithRetry(`/channels/${channelId}/messages?limit=${limit}`);
}

// -- Files --

export async function uploadFile(
  encryptedBlob: Uint8Array,
): Promise<{ file_id: string; size: number }> {
  const token = localStorage.getItem("haven_token");

  const headers: Record<string, string> = {
    "Content-Type": "application/octet-stream",
  };

  if (token) {
    headers["Authorization"] = `Bearer ${token}`;
  }

  const res = await fetch(`${baseUrl}/files`, {
    method: "POST",
    headers,
    body: encryptedBlob as unknown as BodyInit,
  });

  if (!res.ok) {
    const text = await res.text();
    throw new Error(`${res.status}: ${text}`);
  }

  return res.json();
}

export async function downloadFile(fileId: string): Promise<ArrayBuffer> {
  const token = localStorage.getItem("haven_token");

  const headers: Record<string, string> = {};
  if (token) {
    headers["Authorization"] = `Bearer ${token}`;
  }

  // Retry up to 3 attempts for transient failures
  for (let attempt = 0; attempt <= 2; attempt++) {
    const controller = new AbortController();
    const timeout = setTimeout(() => controller.abort(), 30000);

    try {
      const res = await fetch(`${baseUrl}/files/${fileId}`, {
        headers,
        signal: controller.signal,
      });

      if (!res.ok) {
        const text = await res.text();
        throw new Error(`${res.status}: ${text}`);
      }

      return await res.arrayBuffer();
    } catch (e: any) {
      const isRetryable =
        e.name === "AbortError" ||
        (e.message && /^5\d{2}:/.test(e.message)) ||
        e.message?.includes("fetch");
      if (!isRetryable || attempt === 2) throw e;
      await new Promise((r) => setTimeout(r, 500 * Math.pow(2, attempt)));
    } finally {
      clearTimeout(timeout);
    }
  }

  throw new Error("unreachable");
}

// -- Reactions --

export async function toggleReaction(
  channelId: string,
  messageId: string,
  emoji: string,
): Promise<{ added: boolean }> {
  return request(`/channels/${channelId}/messages/${messageId}/reactions`, {
    method: "POST",
    body: JSON.stringify({ emoji }),
  });
}
