// Types and calls for permissions. They mirror app/src-tauri/src/permissions.rs.

import { invoke } from "@tauri-apps/api/core";

export type Status = "allowed" | "denied" | "notAsked" | "unknown";

/** Screen recording at the first check since Debrief started; turning it on takes a restart. */
let screenAtLaunch: Status | null = null;

export type Kind = "microphone" | "screenRecording" | "systemAudio" | "files";

export interface Permissions {
  microphone: Status;
  screenRecording: Status;
  systemAudio: Status;
}

export interface Folder {
  /** With `~` for the home folder. */
  path: string;
  status: Status;
}

export async function loadPermissions(): Promise<Permissions> {
  const permissions = await invoke<Permissions>("permissions");
  screenAtLaunch ??= permissions.screenRecording;
  return permissions;
}
/** Waits for the user to answer the system prompt. */
export const requestPermission = (kind: Kind) => invoke<Status>("request_permission", { kind });
/** Can wait for the answer to macOS's folder prompt. */
export const meetingsFolderAccess = () => invoke<Folder>("meetings_folder_access");
export const openPrivacySettings = (kind: Kind) => invoke<void>("open_privacy_settings", { kind });
export const relaunch = () => invoke<void>("relaunch");

/** Short names for the status line. */
export const shortNames: Record<keyof Permissions, string> = {
  microphone: "Microphone",
  screenRecording: "Screen recording",
  systemAudio: "System audio",
};

/** `unknown` can't be checked, so it doesn't count as missing. */
export function isMissing(status: Status): boolean {
  return status === "denied" || status === "notAsked";
}

export function anyMissing(permissions: Permissions): boolean {
  return Object.values(permissions).some(isMissing);
}

export function needsRestart(permissions: Permissions): boolean {
  return screenAtLaunch !== null && screenAtLaunch !== "allowed" && permissions.screenRecording === "allowed";
}
