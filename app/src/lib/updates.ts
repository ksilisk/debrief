// Types and calls for updates. They mirror app/src-tauri/src/updates.rs.

import { invoke } from "@tauri-apps/api/core";

export type UpdateState =
  | { state: "notChecked" }
  | { state: "checking" }
  | { state: "upToDate" }
  | { state: "available"; version: string }
  | { state: "downloading"; percent: number }
  /** Debrief is about to quit and open the new version. */
  | { state: "installed" }
  | { state: "failed"; message: string };

/** `current` is the running version. */
export type UpdateStatus = UpdateState & { current: string };

/** Sent whenever the status changes. */
export const updateEvent = "update-status";

export const updateStatus = () => invoke<UpdateStatus>("update_status");
export const checkForUpdates = () => invoke<UpdateStatus>("check_for_updates");
/** Quits and opens the new version when it works. */
export const installUpdate = () => invoke<void>("install_update");
