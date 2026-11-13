// Types and calls for recording. They mirror app/src-tauri/src/recording.rs.

import { invoke } from "@tauri-apps/api/core";

export type Language = "auto" | "ru" | "en";

export type VideoSource = { kind: "screen"; id: number } | { kind: "window"; id: number };

export interface Choices {
  title: string;
  /** Off for a talk heard only through the Mac. */
  recordMicrophone: boolean;
  /** A microphone ID; null uses the system default. */
  microphone: string | null;
  /** Off for a meeting in the room or a recording on your own. */
  recordSystemAudio: boolean;
  /** A bundle ID; null records all apps. */
  app: string | null;
  video: boolean;
  fps: number;
  /** null records the main screen. */
  videoSource: VideoSource | null;
  language: Language;
}

export interface Defaults extends Omit<Choices, "title"> {
  /** null until summaries are set up. */
  summaryModel: string | null;
  summaryProvider: "openai" | "anthropic" | null;
}

export interface Choice<T> {
  id: T;
  label: string;
}

export interface SourceList {
  microphones: Choice<string>[];
  apps: Choice<string>[];
  screens: Choice<number>[];
  windows: Choice<number>[];
}

export interface Live {
  folder: string;
  title: string;
  /** null when the microphone is switched off. */
  microphone: string | null;
  /** null when the system audio is switched off. */
  systemAudio: string | null;
  /** The recorded screen or window with its size and frame rate; null for audio only. */
  video: string | null;
}

export interface LiveStatus {
  seconds: number;
  paused: boolean;
  marks: number;
  /** The loudest sample since the last status, from 0 to 1. */
  meLevel: number;
  themLevel: number;
  bytes: number;
}

export interface Saved {
  title: string;
  folder: string;
  problems: string[];
}

/** Events the Rust side sends. */
export const events = {
  started: "recording-started",
  status: "recording-status",
  stopping: "recording-stopping",
  stopped: "recording-stopped",
  failed: "recording-failed",
} as const;

export const recordingDefaults = () => invoke<Defaults>("recording_defaults");
export const recordingSources = () => invoke<SourceList>("recording_sources");
export const currentRecording = () => invoke<Live | null>("current_recording");
export const startRecording = (choices: Choices) => invoke<void>("start_recording", { choices });
export const togglePause = () => invoke<void>("toggle_pause");
export const markMoment = () => invoke<void>("mark_moment");
export const stopRecording = () => invoke<void>("stop_recording");
export const showInFinder = (folder: string) => invoke<void>("show_in_finder", { folder });

/** `HH:MM:SS`, like the menu bar. */
export function clock(seconds: number): string {
  const total = Math.max(0, Math.floor(seconds));
  const pad = (n: number) => String(n).padStart(2, "0");
  return `${pad(Math.floor(total / 3600))}:${pad(Math.floor(total / 60) % 60)}:${pad(total % 60)}`;
}

export function size(bytes: number): string {
  const mb = bytes / 1_000_000;
  if (mb >= 1000) return `${(mb / 1000).toFixed(2)} GB`;
  return mb >= 10 ? `${Math.round(mb)} MB` : `${mb.toFixed(1)} MB`;
}
