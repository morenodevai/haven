import { writable, derived } from "svelte/store";

export interface OnlineUser {
  userId: string;
  username: string;
}

export const onlineUsers = writable<Map<string, OnlineUser>>(new Map());

export function handlePresenceUpdate(event: any) {
  const { user_id, username, online } = event.data;
  onlineUsers.update((m) => {
    if (online) {
      m.set(user_id, { userId: user_id, username });
    } else {
      m.delete(user_id);
    }
    return new Map(m);
  });
}

export const onlineUserList = derived(onlineUsers, ($m) =>
  Array.from($m.values())
);
