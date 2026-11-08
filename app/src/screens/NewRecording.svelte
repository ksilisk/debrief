<script lang="ts">
  import { onMount } from "svelte";
  import InfoTip from "../lib/components/InfoTip.svelte";
  import Toggle from "../lib/components/Toggle.svelte";
  import {
    recordingDefaults,
    recordingSources,
    startRecording,
    type Language,
    type SourceList,
    type VideoSource,
  } from "../lib/recording";

  let { error = $bindable() }: { error: string | null } = $props();

  let title = $state("");
  /** "" uses the system default. */
  let microphone = $state("");
  /** "" records all apps. */
  let app = $state("");
  let video = $state(true);
  let fps = $state(5);
  /** "main", "screen:<id>" or "window:<id>". */
  let videoSource = $state("main");
  let language = $state<Language>("ru");
  let summary = $state(false);
  let summaryModel = $state<string | null>(null);
  let sources = $state<SourceList | null>(null);
  let loading = $state(false);
  let starting = $state(false);

  /** A remembered app that is not running now still works: its sound is recorded once it starts. */
  const appNotRunning = $derived(app !== "" && sources !== null && !sources.apps.some((a) => a.id === app));

  onMount(() => {
    void load();
  });

  async function load() {
    try {
      const defaults = await recordingDefaults();
      microphone = defaults.microphone ?? "";
      app = defaults.app ?? "";
      video = defaults.video;
      fps = defaults.fps;
      videoSource = defaults.videoSource ? `${defaults.videoSource.kind}:${defaults.videoSource.id}` : "main";
      language = defaults.language;
      summary = defaults.summary;
      summaryModel = defaults.summaryModel;
    } catch {
      // Opened in a plain browser (npm run dev) instead of the Tauri window.
      return;
    }
    await refresh();
  }

  async function refresh() {
    loading = true;
    try {
      const list = await recordingSources();
      sources = list;
      if (microphone !== "" && !list.microphones.some((m) => m.id === microphone)) microphone = "";
      if (videoSource !== "main" && !hasVideoSource(list, videoSource)) videoSource = "main";
    } catch (e) {
      error = String(e);
    } finally {
      loading = false;
    }
  }

  function hasVideoSource(list: SourceList, value: string): boolean {
    const source = parseVideoSource(value);
    if (source === null) return true;
    const choices = source.kind === "screen" ? list.screens : list.windows;
    return choices.some((c) => c.id === source.id);
  }

  function parseVideoSource(value: string): VideoSource | null {
    const [kind, id] = value.split(":");
    if ((kind === "screen" || kind === "window") && id !== undefined) return { kind, id: Number(id) };
    return null;
  }

  async function start() {
    starting = true;
    error = null;
    try {
      await startRecording({
        title,
        microphone: microphone === "" ? null : microphone,
        app: app === "" ? null : app,
        video,
        fps,
        videoSource: parseVideoSource(videoSource),
        language,
        summary,
      });
      title = "";
    } catch (e) {
      error = String(e);
    } finally {
      starting = false;
    }
  }
</script>

<div class="screen">
  <label class="field-label" for="title">Meeting title</label>
  <input
    class="title-input"
    id="title"
    type="text"
    bind:value={title}
    placeholder="Leave empty to use the date and time"
    autocomplete="off"
  />

  <div class="sources">
    <div class="src">
      <span class="src-ic me" aria-hidden="true">
        <svg viewBox="0 0 24 24" width="18" height="18" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><rect x="9" y="3" width="6" height="11" rx="3" /><path d="M5 11a7 7 0 0 0 14 0M12 18v3" /></svg>
      </span>
      <div class="src-txt">
        <span class="src-title"><b>Microphone</b><InfoTip label="Microphone" text="Your voice. Saved as its own track and labelled Me in the transcript." /></span>
      </div>
      <div class="src-ctl">
        <span class="select"><select bind:value={microphone} aria-label="Microphone">
          <option value="">System default</option>
          {#each sources?.microphones ?? [] as mic (mic.id)}
            <option value={mic.id}>{mic.label}</option>
          {/each}
        </select></span>
      </div>
    </div>

    <div class="src">
      <span class="src-ic them" aria-hidden="true">
        <svg viewBox="0 0 24 24" width="18" height="18" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><path d="M4 9v6h4l5 4V5L8 9H4z" /><path d="M16.5 8.5a5 5 0 0 1 0 7M19 6a8.5 8.5 0 0 1 0 12" /></svg>
      </span>
      <div class="src-txt">
        <span class="src-title"
          ><b>System audio</b><InfoTip
            label="System audio"
            text="Everyone else: the sound your Mac plays. Saved as its own track and labelled Them. Record all apps, or pick one from the apps running now."
          /></span
        >
      </div>
      <div class="src-ctl">
        <span class="select"><select bind:value={app} aria-label="Record sound from">
          <option value="">All apps</option>
          {#if appNotRunning}
            <option value={app}>{app} (not running)</option>
          {/if}
          {#each sources?.apps ?? [] as a (a.id)}
            <option value={a.id}>{a.label}</option>
          {/each}
        </select></span>
      </div>
    </div>

    <div class="src">
      <span class="src-ic" aria-hidden="true">
        <svg viewBox="0 0 24 24" width="18" height="18" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><rect x="3" y="4" width="18" height="12" rx="2" /><path d="M8 20h8M12 16v4" /></svg>
      </span>
      <div class="src-txt">
        <span class="src-title"
          ><b>Screen video</b><InfoTip
            label="Screen video"
            text="Records one screen or one window. 5 fps is enough for slides; 10 fps suits live demos. Turn it off for audio only."
          /></span
        >
      </div>
      <div class="src-ctl">
        <span class="select"><select bind:value={videoSource} disabled={!video} aria-label="What to record">
          <option value="main">Main screen</option>
          {#if sources && sources.screens.length > 1}
            <optgroup label="Screens">
              {#each sources.screens as screen (screen.id)}
                <option value={`screen:${screen.id}`}>{screen.label}</option>
              {/each}
            </optgroup>
          {/if}
          {#if sources && sources.windows.length > 0}
            <optgroup label="Windows">
              {#each sources.windows as window (window.id)}
                <option value={`window:${window.id}`}>{window.label}</option>
              {/each}
            </optgroup>
          {/if}
        </select></span>
        <div class="seg" role="group" aria-label="Frame rate" aria-disabled={!video}>
          {#each [5, 10] as rate (rate)}
            <button type="button" aria-pressed={fps === rate} disabled={!video} onclick={() => (fps = rate)}>{rate} fps</button>
          {/each}
        </div>
        <Toggle bind:checked={video} label="Record screen video" />
      </div>
    </div>

    <div class="src">
      <span class="src-ic" aria-hidden="true">
        <svg viewBox="0 0 24 24" width="18" height="18" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><circle cx="12" cy="12" r="9" /><path d="M3 12h18M12 3a14 14 0 0 1 0 18M12 3a14 14 0 0 0 0 18" /></svg>
      </span>
      <div class="src-txt">
        <span class="src-title"
          ><b>Language</b><InfoTip
            label="Language"
            text="Auto finds one language for each track from its first 30 seconds, so each side can speak its own language. Pick a language when the whole meeting is in it."
          /></span
        >
      </div>
      <div class="src-ctl">
        <span class="select"><select bind:value={language} aria-label="Language">
          <option value="auto">Auto</option>
          <option value="ru">Russian</option>
          <option value="en">English</option>
        </select></span>
      </div>
    </div>

    <div class="src">
      <span class="src-ic" aria-hidden="true">
        <svg viewBox="0 0 24 24" width="18" height="18" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><path d="M6 3h9l3 3v15H6z" /><path d="M9 10h6M9 14h6M9 18h4" /></svg>
      </span>
      <div class="src-txt">
        <span class="src-title"
          ><b>Summary</b><InfoTip
            label="Summary"
            text="Turn off to get only the transcript for this recording. The default and the provider are set in Settings."
          /></span
        >
        <span>{summaryModel ?? "Not set up yet"}</span>
      </div>
      <div class="src-ctl">
        <Toggle bind:checked={summary} label="Write a summary after the transcript" disabled={summaryModel === null} />
      </div>
    </div>
  </div>

  <div class="refresh">
    <button type="button" class="link" onclick={refresh} disabled={loading}>
      {loading ? "Looking for microphones, apps and windows…" : "Refresh the lists"}
    </button>
  </div>

  {#if error}
    <p class="error" role="alert">{error}</p>
  {/if}

  <div class="rec-row">
    <button type="button" class="rec-btn" aria-label="Start recording" onclick={start} disabled={starting}><span></span></button>
    <div>
      <b>{starting ? "Starting…" : "Start recording"}</b>
      <span class="hint">or press <kbd>⌃</kbd> <kbd>⌘</kbd> <kbd>R</kbd> from any app</span>
    </div>
  </div>
</div>

<style>
  .screen {
    max-width: 760px;
  }

  .field-label {
    display: block;
    margin-bottom: 4px;
    font-size: 12px;
    font-weight: 600;
    color: var(--muted);
  }

  .title-input {
    width: 100%;
    padding: 4px 0 8px;
    border: 0;
    border-bottom: 1px solid var(--line);
    background: transparent;
    color: var(--ink);
    font: inherit;
    font-size: 22px;
    font-weight: 700;
    outline: none;
  }

  .title-input:focus-visible {
    border-bottom-color: var(--me);
  }

  .sources {
    margin-top: 20px;
    border: 1px solid var(--line);
    border-radius: 10px;
  }

  .src {
    display: grid;
    grid-template-columns: 36px minmax(150px, 1fr) auto;
    gap: 14px;
    align-items: center;
    padding: 12px 14px;
  }

  .src + .src {
    border-top: 1px solid var(--line);
  }

  .src-ic {
    width: 36px;
    height: 36px;
    border-radius: 9px;
    display: grid;
    place-items: center;
    background: var(--soft);
    color: var(--muted);
  }

  .src-ic.me {
    color: var(--me);
    background: color-mix(in srgb, var(--me) 13%, transparent);
  }

  .src-ic.them {
    color: var(--them);
    background: color-mix(in srgb, var(--them) 13%, transparent);
  }

  .src-txt {
    display: grid;
    gap: 1px;
    min-width: 0;
  }

  .src-txt > span:not(.src-title) {
    font-size: 12px;
    color: var(--muted);
  }

  .src-title {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    font-size: 14px;
  }

  .src-ctl {
    display: flex;
    gap: 8px;
    align-items: center;
    justify-content: flex-end;
    flex-wrap: wrap;
  }

  /* A flat field with a chevron, as in the design, instead of the macOS pop-up button. */
  .select {
    position: relative;
    display: inline-flex;
    max-width: 260px;
  }

  .select::after {
    content: "";
    position: absolute;
    top: 50%;
    right: 10px;
    width: 5px;
    height: 5px;
    border-right: 1.5px solid var(--muted);
    border-bottom: 1.5px solid var(--muted);
    transform: translateY(-75%) rotate(45deg);
    pointer-events: none;
  }

  select {
    appearance: none;
    -webkit-appearance: none;
    min-width: 0;
    max-width: 260px;
    padding: 5px 26px 5px 8px;
    text-overflow: ellipsis;
    border: 1px solid var(--line);
    border-radius: 7px;
    background: var(--surface);
    color: var(--ink);
    font: inherit;
    font-size: 13px;
  }

  select:disabled,
  .select:has(select:disabled)::after {
    opacity: 0.45;
  }

  .seg {
    display: inline-flex;
    border: 1px solid var(--line);
    border-radius: 7px;
    overflow: hidden;
  }

  .seg[aria-disabled="true"] {
    opacity: 0.45;
  }

  .seg button {
    padding: 5px 10px;
    border: 0;
    background: var(--surface);
    color: var(--muted);
    font: inherit;
    font-size: 12px;
    font-weight: 600;
    cursor: pointer;
  }

  .seg button + button {
    border-left: 1px solid var(--line);
  }

  .seg button[aria-pressed="true"] {
    background: var(--soft);
    color: var(--ink);
  }

  .refresh {
    margin-top: 6px;
    text-align: right;
  }

  .link {
    padding: 0;
    border: 0;
    background: none;
    color: var(--muted);
    font: inherit;
    font-size: 12px;
    cursor: pointer;
  }

  .link:hover:not(:disabled) {
    color: var(--ink);
    text-decoration: underline;
  }

  .error {
    margin: 18px 0 0;
    padding: 10px 12px;
    border-radius: 8px;
    border: 1px solid color-mix(in srgb, var(--rec) 40%, var(--line));
    background: color-mix(in srgb, var(--rec) 7%, var(--surface));
    font-size: 13px;
  }

  .rec-row {
    display: flex;
    align-items: center;
    gap: 16px;
    margin-top: 24px;
  }

  .rec-btn {
    width: 66px;
    height: 66px;
    flex: none;
    border-radius: 50%;
    display: grid;
    place-items: center;
    padding: 0;
    cursor: pointer;
    border: 3px solid color-mix(in srgb, var(--rec) 28%, var(--line));
    background: var(--surface);
  }

  .rec-btn span {
    width: 42px;
    height: 42px;
    border-radius: 50%;
    background: var(--rec);
    transition: transform 0.15s;
  }

  .rec-btn:hover:not(:disabled) span {
    transform: scale(1.07);
  }

  .rec-btn:disabled {
    cursor: default;
    opacity: 0.6;
  }

  .rec-row b {
    display: block;
    font-size: 16px;
  }

  .hint {
    font-size: 12px;
    color: var(--muted);
  }

  kbd {
    font: 500 11px var(--font-mono);
    padding: 0 5px;
    border-radius: 4px;
    border: 1px solid var(--line);
    border-bottom-width: 2px;
    background: var(--surface);
    color: var(--ink);
  }
</style>
