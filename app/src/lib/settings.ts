// Types and calls for the Settings screen. They mirror app/src-tauri/src/settings.rs.

import { invoke } from "@tauri-apps/api/core";

export type Provider = "openai" | "anthropic";

export interface SettingsView {
  /** With `~` for the home folder. */
  meetingsFolder: string;
  video: boolean;
  fps: number;
  hints: string;
  /** null until the Whisper model is downloaded. */
  hintTokens: number | null;
  maxHintTokens: number;
  whisper: { name: string; bytes: number; downloaded: boolean };
  summary: {
    /** null until summaries are set up. */
    provider: Provider | null;
    model: string;
    contextTokens: number;
    onByDefault: boolean;
    /** The saved keys, masked. */
    keys: Record<Provider, string | null>;
  };
}

/** What changed; anything left out keeps its saved value. */
export interface Changes {
  video?: boolean;
  fps?: number;
  hints?: string;
  summariesOn?: boolean;
  contextTokens?: number;
}

export interface ModelView {
  id: string;
  /** Known for Claude models; OpenAI doesn't list it. */
  contextTokens: number | null;
  answerTokens: number;
  suggested: boolean;
}

export interface ModelChoice {
  provider: Provider;
  model: string;
  contextTokens: number;
  /** null uses the default limit. */
  answerTokens: number | null;
}

export const providers: { id: Provider; label: string; host: string }[] = [
  { id: "openai", label: "OpenAI", host: "api.openai.com" },
  { id: "anthropic", label: "Claude (Anthropic)", host: "api.anthropic.com" },
];

export const loadSettings = () => invoke<SettingsView>("settings");
export const saveSettings = (changes: Changes) => invoke<void>("save_settings", { changes });
export const countHintTokens = (hints: string) => invoke<number | null>("count_hint_tokens", { hints });
export const chooseMeetingsFolder = () => invoke<string | null>("choose_meetings_folder");
export const summaryModels = (provider: Provider, key: string | null) =>
  invoke<ModelView[]>("summary_models", { provider, key });
export const useSummaryModel = (choice: ModelChoice) => invoke<void>("use_summary_model", { choice });
export const editSummaryPrompt = () => invoke<void>("edit_summary_prompt");

/** `128k tokens`, `1M tokens`. */
export function tokens(count: number): string {
  if (count >= 1_000_000 && count % 1_000_000 === 0) return `${count / 1_000_000}M tokens`;
  if (count >= 1_000 && count % 1_000 === 0) return `${count / 1_000}k tokens`;
  return `${count} tokens`;
}
