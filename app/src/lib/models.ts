// Types and calls for the Whisper models. They mirror app/src-tauri/src/speech_models.rs.

import { invoke } from "@tauri-apps/api/core";

export interface ModelInfo {
  name: string;
  bytes: number;
  downloaded: boolean;
  /** What an unfinished download already has. */
  partialBytes: number;
}

export interface DownloadInfo {
  model: string;
  done: number;
  total: number;
  secondsLeft: number | null;
}

export interface SpeechModels {
  /** The model that transcribes. */
  chosen: string;
  models: ModelInfo[];
  download: DownloadInfo | null;
  /** Why the last download failed. */
  error: string | null;
}

/** Sent whenever a download moves on or a model changes. */
export const speechModelsEvent = "speech-models";

export const loadSpeechModels = () => invoke<SpeechModels>("speech_models");
export const downloadModel = (name: string) => invoke<void>("download_model", { name });
export const cancelModelDownload = () => invoke<void>("cancel_model_download");
export const chooseModel = (name: string) => invoke<void>("choose_model", { name });
export const deleteModel = (name: string) => invoke<void>("delete_model", { name });

export const modelNames: Record<string, { label: string; note: string }> = {
  "large-v3-turbo": { label: "Large v3 Turbo", note: "Accurate and fast. The default." },
  "large-v3-turbo-q5_0": { label: "Large v3 Turbo, compressed", note: "A third of the size, nearly as accurate." },
  "large-v3": { label: "Large v3", note: "The most accurate, and several times slower." },
};

export function modelLabel(name: string): string {
  return modelNames[name]?.label ?? name;
}

export function chosenModel(models: SpeechModels): ModelInfo | undefined {
  return models.models.find((model) => model.name === models.chosen);
}

export function timeLeft(seconds: number | null): string {
  if (seconds === null) return "";
  if (seconds < 60) return "less than a minute left";
  const minutes = Math.round(seconds / 60);
  return minutes < 60 ? `about ${minutes} min left` : `about ${Math.floor(minutes / 60)} h ${minutes % 60} min left`;
}
