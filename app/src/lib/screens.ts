/** The window's screens. The Rust side opens them by these names (`Screen` in window.rs). */
export type Screen = "new" | "recording" | "processing" | "meeting" | "settings";

export const screenTitles: Record<Screen, string> = {
  new: "New recording",
  recording: "Recording",
  processing: "Processing",
  meeting: "Meeting",
  settings: "Settings",
};
