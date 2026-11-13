// Types and calls for the meeting list, one meeting and its processing.
// They mirror app/src-tauri/src/library.rs and processing.rs.

import { invoke } from "@tauri-apps/api/core";

export type Status =
  | "recording"
  | "processing"
  | "waiting"
  | "summarizing"
  | "ready"
  | "transcriptOnly"
  | "notProcessed"
  | "cutOff"
  | "failed";

export type Step = "transcribe" | "merge" | "export" | "summary";
export type StepStatus = "pending" | "running" | "done" | "skipped" | "failed";
export type Speaker = "me" | "them";

export interface MeetingItem {
  folder: string;
  title: string;
  /** RFC 3339. */
  startedAt: string;
  durationSeconds: number;
  status: Status;
}

export interface StepView {
  step: Step;
  status: StepStatus;
  error: string | null;
  model: string | null;
}

export interface Segment {
  start: number;
  speaker: Speaker;
  text: string;
  echo: boolean;
}

export interface MeetingDetails extends MeetingItem {
  video: string | null;
  language: string;
  heard: string[];
  steps: StepView[];
  summary: string | null;
  /** The meeting's own summary prompt; null when it uses the default. */
  ownPrompt: string | null;
  /** The prompt from Settings. */
  defaultPrompt: string;
  segments: Segment[];
  /** Whether lines say who spoke: only when both tracks have lines. */
  labelled: boolean;
  /** The one track recorded when the other was switched off. */
  onlyTrack: Speaker | null;
  marks: number[];
  files: { name: string; bytes: number }[];
  /** RFC 3339; when the storage cleanup deleted the audio and video. */
  filesDeletedAt: string | null;
}

export interface Progress {
  folder: string;
  title: string;
  steps: { step: Step; status: StepStatus }[];
  track: Speaker | null;
  trackDone: number;
  percent: number;
  secondsLeft: number | null;
  summaryCharacters: number;
  transcribeModel: string;
  summaryModel: string | null;
  /** The job writes the summary only. */
  summaryOnly: boolean;
}

export interface Finished {
  folder: string;
  title: string;
  problem: string | null;
  cancelled: boolean;
}

/** Events the Rust side sends. */
export const meetingEvents = {
  changed: "meetings-changed",
  progress: "processing-progress",
  finished: "processing-finished",
  open: "open-meeting",
} as const;

export const listMeetings = () => invoke<MeetingItem[]>("meetings");
export const meetingDetails = (folder: string) => invoke<MeetingDetails>("meeting", { folder });
export const processMeeting = (folder: string) => invoke<void>("process_meeting", { folder });
/** `prompt` becomes the meeting's own; null makes it use the default again. */
export const writeSummary = (folder: string, prompt: string | null) =>
  invoke<void>("write_summary", { folder, prompt });
export const stopProcessing = (folder: string) => invoke<void>("stop_processing", { folder });
export const processingProgress = () => invoke<Progress | null>("processing_progress");
export const copyText = (text: string) => invoke<void>("copy_text", { text });
/** Asks first; false when nothing was moved. Problems show in an alert. */
export const moveToTrash = (folder: string) => invoke<boolean>("delete_meeting", { folder });

export const statusLabels: Record<Status, string> = {
  recording: "Recording",
  processing: "Processing",
  waiting: "Waiting",
  summarizing: "Writing summary",
  ready: "Ready",
  transcriptOnly: "Transcript only",
  notProcessed: "Not processed",
  cutOff: "Cut off",
  failed: "Failed",
};

/** "47 min", "1 h 12 min" or "38 s". */
export function duration(seconds: number): string {
  if (seconds < 60) return `${Math.round(seconds)} s`;
  const minutes = Math.round(seconds / 60);
  return minutes >= 60 ? `${Math.floor(minutes / 60)} h ${minutes % 60} min` : `${minutes} min`;
}

/** "14:30" in the recording's own time zone offset, as the folder name has it. */
export function timeOfDay(startedAt: string): string {
  return startedAt.slice(11, 16);
}

/** "Today", "Yesterday" or "Mon, 2 Nov", for grouping the list. */
export function dayLabel(startedAt: string, now = new Date()): string {
  const day = startedAt.slice(0, 10);
  const local = (date: Date) =>
    `${date.getFullYear()}-${String(date.getMonth() + 1).padStart(2, "0")}-${String(date.getDate()).padStart(2, "0")}`;
  const yesterday = new Date(now);
  yesterday.setDate(now.getDate() - 1);
  if (day === local(now)) return "Today";
  if (day === local(yesterday)) return "Yesterday";
  const date = new Date(`${day}T12:00:00`);
  return date.toLocaleDateString("en-GB", { weekday: "short", day: "numeric", month: "short" });
}

/** "Sun 8 Nov, 22:48". */
export function longDate(startedAt: string): string {
  const date = new Date(`${startedAt.slice(0, 10)}T12:00:00`);
  const day = date.toLocaleDateString("en-GB", { weekday: "short", day: "numeric", month: "short" }).replace(",", "");
  return `${day}, ${timeOfDay(startedAt)}`;
}

const languageNames: Record<string, string> = { ru: "Russian", en: "English", auto: "Auto" };

export function languageName(code: string): string {
  return languageNames[code] ?? code;
}
