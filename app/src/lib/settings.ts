// Types and calls for the Settings screen. They mirror app/src-tauri/src/settings.rs.

import { invoke } from "@tauri-apps/api/core";

export type Provider = "openai" | "anthropic" | "openrouter" | "server";
/** The providers with a fixed address and a required key. */
export type Hosted = Exclude<Provider, "server">;
export type Delete = "recordings" | "meeting";

export interface SettingsView {
  /** With `~` for the home folder. */
  meetingsFolder: string;
  video: boolean;
  fps: number;
  whisper: { name: string; bytes: number; downloaded: boolean };
  summary: {
    /** null until summaries are set up. */
    provider: Provider | null;
    /** With the "server" provider. */
    server: number | null;
    model: string;
    contextTokens: number;
    /** The saved keys, masked. */
    keys: Record<Hosted, string | null>;
    servers: ServerView[];
  };
  cleanup: {
    /** null never deletes anything. */
    afterDays: number | null;
    delete: Delete;
    /** What the meetings take now. */
    usage: { meetings: number; bytes: number };
  };
}

/** What changed; anything left out keeps its saved value. */
export interface Changes {
  video?: boolean;
  fps?: number;
  contextTokens?: number;
}

export interface ServerView {
  id: number;
  label: string;
  url: string;
  /** The label, or the host. */
  name: string;
  /** Masked; null when the server has no key. */
  key: string | null;
}

/** What Test connection checks; a server as typed, with a null id when it's new. */
export interface Target {
  provider: Provider;
  server: { id: number | null; label: string; url: string } | null;
}

export interface ModelList {
  models: ModelView[];
  /** The server's id; a new server has one now. */
  server: number | null;
}

export interface ModelView {
  id: string;
  /** Listed by Claude, OpenRouter and some servers; not by OpenAI. */
  contextTokens: number | null;
  answerTokens: number;
  suggested: boolean;
}

export interface ModelChoice {
  provider: Provider;
  /** With the "server" provider. */
  server: number | null;
  model: string;
  contextTokens: number;
  /** null uses the default limit. */
  answerTokens: number | null;
}

export const providers: { id: Hosted; label: string; url: string }[] = [
  { id: "openai", label: "OpenAI", url: "https://api.openai.com/v1" },
  { id: "anthropic", label: "Claude (Anthropic)", url: "https://api.anthropic.com/v1" },
  { id: "openrouter", label: "OpenRouter", url: "https://openrouter.ai/api/v1" },
];

/** Where an API's base URL sends the transcript; null for a URL that isn't one. */
export function destination(url: string): { host: string; local: boolean; encrypted: boolean } | null {
  let parsed: URL;
  try {
    parsed = new URL(url.trim());
  } catch {
    return null;
  }
  if ((parsed.protocol !== "http:" && parsed.protocol !== "https:") || parsed.hostname === "") return null;
  const name = parsed.hostname.toLowerCase();
  const local =
    name === "localhost" ||
    name.endsWith(".localhost") ||
    name === "[::1]" ||
    /^127\.\d+\.\d+\.\d+$/.test(name);
  return { host: parsed.host, local, encrypted: parsed.protocol === "https:" };
}

export const loadSettings = () => invoke<SettingsView>("settings");
export const saveSettings = (changes: Changes) => invoke<void>("save_settings", { changes });
export const chooseMeetingsFolder = () => invoke<string | null>("choose_meetings_folder");
export const summaryModels = (target: Target, key: string | null) =>
  invoke<ModelList>("summary_models", { target, key });
export const useSummaryModel = (choice: ModelChoice) => invoke<void>("use_summary_model", { choice });
export const renameServer = (id: number, label: string) => invoke<void>("rename_server", { id, label });
export const removeServer = (id: number) => invoke<void>("remove_server", { id });
export const editSummaryPrompt = () => invoke<void>("edit_summary_prompt");
/** Shows the log file in Finder. */
export const showLog = () => invoke<void>("show_log");
/** false when the user chose not to delete what the new setting applies to now. */
export const setCleanup = (afterDays: number | null, del: Delete) =>
  invoke<boolean>("set_cleanup", { afterDays, delete: del });

/** "30 days", "1 year". */
export function age(days: number): string {
  if (days === 365) return "1 year";
  return days === 1 ? "1 day" : `${days} days`;
}

/** `128k tokens`, `1M tokens`. */
export function tokens(count: number): string {
  if (count >= 1_000_000 && count % 1_000_000 === 0) return `${count / 1_000_000}M tokens`;
  if (count >= 1_000 && count % 1_000 === 0) return `${count / 1_000}k tokens`;
  return `${count} tokens`;
}
