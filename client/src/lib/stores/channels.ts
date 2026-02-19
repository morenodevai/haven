import { writable } from "svelte/store";

export type ChannelView = "general" | "file-sharing";

export const activeChannel = writable<ChannelView>("general");
