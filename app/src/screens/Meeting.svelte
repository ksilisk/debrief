<script lang="ts">
  import ModelDownload from "../lib/components/ModelDownload.svelte";
  import { chosenModel, type SpeechModels } from "../lib/models";
  import { onMount } from "svelte";
  import { clock, recordingDefaults, showInFinder } from "../lib/recording";
  import { renderMarkdown } from "../lib/markdown";
  import {
    copyText,
    duration,
    languageName,
    longDate,
    meetingDetails,
    moveToTrash,
    processMeeting,
    stopProcessing,
    writeSummary,
    type MeetingDetails,
    type Progress,
    type Segment,
    type Step,
    type StepStatus,
  } from "../lib/meetings";

  let {
    folder,
    progress,
    version,
    problems = [],
    models,
    onSettings,
  }: {
    folder: string;
    /** The live progress when this meeting is the one being processed. */
    progress: Progress | null;
    /** Goes up when meetings change, so the details load again. */
    version: number;
    /** What went wrong while saving the recording. */
    problems?: string[];
    models: SpeechModels | null;
    onSettings: () => void;
  } = $props();

  const modelMissing = $derived(models !== null && chosenModel(models)?.downloaded === false);

  let details = $state<MeetingDetails | null>(null);
  let loadError = $state<string | null>(null);
  let actionError = $state<string | null>(null);
  let tab = $state<"summary" | "transcript" | "files">("summary");
  let showEcho = $state(false);
  let copied = $state(false);
  let summaryModel = $state<string | null>(null);

  $effect(() => {
    void load(folder, version);
  });

  // Each meeting opens on its summary.
  $effect(() => {
    void folder;
    tab = "summary";
    showEcho = false;
  });

  onMount(() => {
    recordingDefaults()
      .then((defaults) => (summaryModel = defaults.summaryModel))
      .catch(() => {
        // Opened in a plain browser (npm run dev) instead of the Tauri window.
      });
  });

  async function load(target: string, _version: number) {
    try {
      const loaded = await meetingDetails(target);
      if (target === folder) {
        details = loaded;
        loadError = null;
      }
    } catch (e) {
      loadError = String(e);
    }
  }

  async function run(action: () => Promise<void>) {
    actionError = null;
    try {
      await action();
    } catch (e) {
      actionError = String(e);
    }
  }

  async function copySummary() {
    if (!details?.summary) return;
    await run(() => copyText(details?.summary ?? ""));
    if (actionError) return;
    copied = true;
    setTimeout(() => (copied = false), 2000);
  }

  const busy = $derived(details?.status === "processing" || details?.status === "waiting");
  const transcribed = $derived(details?.status === "ready" || details?.status === "transcriptOnly");
  const stepOf = (step: Step) => details?.steps.find((entry) => entry.step === step);
  const summaryFailed = $derived(stepOf("summary")?.status === "failed");
  const failure = $derived(details?.steps.find((entry) => entry.status === "failed")?.error ?? null);
  const echoCount = $derived(details?.segments.filter((segment) => segment.echo).length ?? 0);

  const subtitle = $derived.by(() => {
    if (!details) return "";
    const fps = details.video ? /(\d+) fps/.exec(details.video)?.[1] : null;
    const languages = details.heard.length > 0 ? details.heard : [details.language];
    return [
      longDate(details.startedAt),
      duration(details.durationSeconds),
      details.video ? `audio + video${fps ? ` ${fps} fps` : ""}` : "audio only",
      languages.map(languageName).join(" + "),
    ].join(" · ");
  });

  type Row = { kind: "line"; segment: Segment } | { kind: "mark"; at: number };
  const rows = $derived.by((): Row[] => {
    if (!details) return [];
    const marks = [...details.marks].sort((a, b) => a - b);
    const result: Row[] = [];
    let next = 0;
    for (const segment of details.segments) {
      while (next < marks.length && marks[next] <= segment.start) result.push({ kind: "mark", at: marks[next++] });
      if (showEcho || !segment.echo) result.push({ kind: "line", segment });
    }
    while (next < marks.length) result.push({ kind: "mark", at: marks[next++] });
    return result;
  });

  // The processing view: live steps when this meeting is being processed, saved ones while it waits.
  const steps = $derived(
    progress?.steps ?? details?.steps.map((entry) => ({ step: entry.step, status: entry.status })) ?? [],
  );
  const statusOf = (step: Step): StepStatus => steps.find((entry) => entry.step === step)?.status ?? "pending";

  function timeLeft(seconds: number | null): string {
    if (seconds === null) return "";
    return seconds < 60 ? " · less than a minute left" : ` · about ${Math.round(seconds / 60)} min left`;
  }

  function transcribeDetail(): string {
    const model = `Whisper ${progress?.transcribeModel ?? "large-v3-turbo"}`;
    if (statusOf("transcribe") !== "running" || !progress?.track) return model;
    const track = progress.track === "me" ? "Me" : "Them";
    return `${track} · ${Math.round(progress.trackDone * 100)}%${timeLeft(progress.secondsLeft)} · ${model}`;
  }

  function summaryDetail(): string {
    const status = statusOf("summary");
    if (status === "skipped") return "Summaries are off for this meeting";
    const model = progress?.summaryModel ?? summaryModel;
    if (status === "running") return `${progress?.summaryCharacters ?? 0} characters so far${model ? ` · ${model}` : ""}`;
    return model ?? "Summaries are not set up";
  }

  const fileNotes: Record<string, string> = {
    "summary.md": "Same summary with headings",
    "summary.txt": "For people who don’t use Markdown",
    "transcript.md": "Me and Them with timecodes",
    "transcript.txt": "Plain text for anyone",
    "transcript.srt": "Subtitles for screen.mp4",
    "me.wav": "Your microphone, untouched",
    "them.wav": "System audio, untouched",
    "screen.mp4": "Screen video",
    "meeting.json": "Settings, marked moments, model versions",
    "transcript.json": "Every line with its time, speaker and language",
  };

  function fileNote(name: string): string {
    if (/^screen-\d+\.mp4$/.test(name)) return "A video part that could not be joined";
    if (name === "transcript.srt" && details?.filesDeletedAt) return "Subtitles";
    return fileNotes[name] ?? "";
  }

  function fileSize(bytes: number): string {
    if (bytes >= 1e9) return `${(bytes / 1e9).toFixed(1)} GB`;
    if (bytes >= 1e6) return `${Math.round(bytes / 1e6)} MB`;
    return `${Math.max(1, Math.round(bytes / 1e3))} KB`;
  }
</script>

{#if loadError && !details}
  <p class="error" role="alert">{loadError}</p>
{:else if details}
  <div class="m-head">
    <div>
      <h2>{details.title}</h2>
      <p class="sub">{subtitle}</p>
    </div>
    <div class="m-actions">
      {#if details.summary && !busy}
        <button type="button" class="btn primary" onclick={copySummary}>{copied ? "Copied" : "Copy summary"}</button>
      {/if}
      <button type="button" class="btn" onclick={() => run(() => showInFinder(details?.folder ?? ""))}>Show in Finder</button>
      {#if !busy && details.status !== "recording"}
        <button type="button" class="btn" onclick={() => run(async () => void (await moveToTrash(folder)))}>
          Move to Trash
        </button>
      {/if}
    </div>
  </div>

  {#each problems as problem (problem)}
    <p class="note warn">{problem}</p>
  {/each}
  {#if actionError}
    <p class="error" role="alert">{actionError}</p>
  {/if}

  {#if busy}
    {#if details.status === "waiting"}
      <p class="sub waiting">Waiting for another meeting to finish processing.</p>
    {/if}
    <ol class="steps">
      <li class="done">
        <span class="st"></span>
        <div><b>Saved the recording</b><span class="d">In the meeting folder</span></div>
      </li>
      <li class={statusOf("transcribe")}>
        <span class="st"></span>
        <div>
          <b>Transcribing on this Mac</b><span class="d">{transcribeDetail()}</span>
          {#if statusOf("transcribe") === "running"}
            <div class="bar"><i style:width={`${Math.round((progress?.trackDone ?? 0) * 100)}%`}></i></div>
          {/if}
        </div>
      </li>
      <li class={statusOf("merge")}>
        <span class="st"></span>
        <div>
          <b>Merging Me and Them, removing echo lines</b><span class="d">Lines both tracks heard at the same moment are dropped from Me</span>
        </div>
      </li>
      <li class={statusOf("export")}>
        <span class="st"></span>
        <div><b>Saving the transcript files</b><span class="d">Into the meeting folder</span></div>
      </li>
      <li class={statusOf("summary")}>
        <span class="st"></span>
        <div><b>Writing the summary</b><span class="d">{summaryDetail()}</span></div>
      </li>
    </ol>
    <p class="sub">You can close the window. Processing continues, and a notification tells you when the files are ready.</p>
    <div class="ctrls">
      <button type="button" class="btn" onclick={() => run(() => stopProcessing(folder))}>
        {details.status === "waiting" ? "Take out of the queue" : "Stop processing"}
      </button>
    </div>
  {:else if !transcribed}
    {#if details.status !== "recording"}
      <ModelDownload {models} {onSettings} />
    {/if}
    <div class="empty">
      {#if details.status === "recording"}
        <b>This meeting is being recorded</b>
      {:else}
        <b>{details.status === "failed" ? "Processing failed" : "Not transcribed yet"}</b>
        <p>
          {#if details.status === "failed" && failure}
            {failure}
          {:else if details.status === "cutOff"}
            The recording was cut off when the app quit. Debrief repairs it first, then transcribes it.
          {:else if modelMissing}
            Debrief transcribes it as soon as a speech model is downloaded.
          {:else}
            Debrief transcribes it on this Mac and writes the files into the meeting folder.
          {/if}
        </p>
        <button type="button" class="btn" onclick={() => run(() => processMeeting(folder))}>
          {details.status === "failed" ? "Try again" : "Process now"}
        </button>
      {/if}
    </div>
  {:else}
    <div class="tabs" role="tablist" aria-label="Meeting content">
      <button type="button" role="tab" aria-selected={tab === "summary"} onclick={() => (tab = "summary")}>Summary</button>
      <button type="button" role="tab" aria-selected={tab === "transcript"} onclick={() => (tab = "transcript")}>Transcript</button>
      <button type="button" role="tab" aria-selected={tab === "files"} onclick={() => (tab = "files")}>Files</button>
    </div>

    {#if tab === "summary"}
      <div role="tabpanel">
        {#if details.summary}
          <!-- renderMarkdown escapes the text and adds only its own tags. -->
          <div class="sumdoc">{@html renderMarkdown(details.summary)}</div>
        {:else}
          <div class="empty">
            <b>{summaryFailed ? "The summary was not written" : "No summary for this meeting"}</b>
            <p>
              {#if summaryFailed}
                {stepOf("summary")?.error ?? "Writing it failed."}
              {:else}
                Summaries were off when it was recorded. The transcript{details.filesDeletedAt ? "" : " and audio"}
                {details.filesDeletedAt ? "is" : "are"} in the meeting folder.
              {/if}
            </p>
            {#if summaryModel}
              <button type="button" class="btn" onclick={() => run(() => writeSummary(folder))}>Write summary now</button>
            {:else}
              <p>Summaries are not set up yet. Run <code>debrief config summaries</code> in Terminal first.</p>
            {/if}
          </div>
        {/if}
      </div>
    {:else if tab === "transcript"}
      <div role="tabpanel">
        {#if details.segments.length === 0}
          <p class="sub">No speech was found in this recording.</p>
        {:else}
          <div class="tx">
            {#each rows as row, i (i)}
              {#if row.kind === "mark"}
                <div class="tx-mark">★ {clock(row.at)} · Marked moment</div>
              {:else}
                <div class="tx-line" class:echo={row.segment.echo}>
                  <span class="tc">{clock(row.segment.start)}</span>
                  <span class="who {row.segment.speaker}">{row.segment.speaker === "me" ? "Me" : "Them"}</span>
                  <span class="say">{row.segment.text}{#if row.segment.echo}<span class="tag">echo, removed</span>{/if}</span>
                </div>
              {/if}
            {/each}
          </div>
          {#if echoCount > 0}
            <div class="tx-foot">
              <span>{echoCount} {echoCount === 1 ? "line" : "lines"} that both tracks heard {echoCount === 1 ? "was" : "were"} removed from Me.</span>
              <button type="button" class="btn small" aria-pressed={showEcho} onclick={() => (showEcho = !showEcho)}>
                {showEcho ? "Hide removed lines" : "Show removed lines"}
              </button>
            </div>
          {/if}
        {/if}
      </div>
    {:else}
      <div role="tabpanel" class="files-wrap">
        <table class="files">
          <tbody>
            {#each details.files as file (file.name)}
              <tr>
                <td class:me={file.name === "me.wav"} class:them={file.name === "them.wav"}>{file.name}</td>
                <td class="what">{fileNote(file.name)}</td>
                <td class="sz">{fileSize(file.bytes)}</td>
              </tr>
            {/each}
          </tbody>
        </table>
        {#if details.filesDeletedAt}
          <p class="sub files-note">The audio and video went to the Trash on {longDate(details.filesDeletedAt)}.</p>
        {/if}
      </div>
    {/if}
  {/if}
{/if}

<style>
  h2 {
    margin: 0;
    font-size: 22px;
    font-weight: 700;
    letter-spacing: -0.01em;
  }

  .sub {
    margin: 4px 0 0;
    font-size: 12px;
    color: var(--muted);
  }

  .waiting {
    margin-top: 16px;
  }

  .m-head {
    display: flex;
    gap: 16px;
    justify-content: space-between;
    align-items: flex-start;
    flex-wrap: wrap;
  }

  .m-actions {
    display: flex;
    gap: 8px;
    flex-wrap: wrap;
  }

  .btn {
    display: inline-flex;
    align-items: center;
    gap: 8px;
    padding: 8px 14px;
    border-radius: 8px;
    border: 1px solid var(--line);
    background: var(--surface);
    color: var(--ink);
    font: inherit;
    font-size: 13px;
    font-weight: 600;
    cursor: pointer;
  }

  .btn:hover {
    background: var(--soft);
  }

  .btn.primary {
    background: var(--ink);
    border-color: var(--ink);
    color: var(--surface);
  }

  .btn.primary:hover {
    opacity: 0.9;
  }

  .btn.small {
    padding: 4px 10px;
    font-size: 12px;
  }

  .note,
  .error {
    max-width: 72ch;
    margin: 16px 0 0;
    padding: 10px 12px;
    border-radius: 8px;
    font-size: 13px;
  }

  .note.warn {
    border: 1px solid color-mix(in srgb, var(--warn) 45%, var(--line));
    background: color-mix(in srgb, var(--warn) 8%, var(--surface));
  }

  .error {
    border: 1px solid color-mix(in srgb, var(--rec) 40%, var(--line));
    background: color-mix(in srgb, var(--rec) 7%, var(--surface));
  }

  /* Processing steps */
  .steps {
    list-style: none;
    margin: 24px 0 18px 9px;
    padding: 0;
    border-left: 2px solid var(--line);
  }

  .steps li {
    display: grid;
    grid-template-columns: 20px minmax(0, 1fr);
    gap: 12px;
    margin-left: -11px;
    padding: 0 0 16px;
  }

  .steps li:last-child {
    padding-bottom: 0;
  }

  .st {
    width: 20px;
    height: 20px;
    border-radius: 50%;
    display: grid;
    place-items: center;
    background: var(--surface);
    border: 2px solid var(--line);
  }

  .steps li.done .st {
    background: var(--ok);
    border-color: var(--ok);
  }

  .steps li.done .st::after {
    content: "";
    width: 5px;
    height: 9px;
    border: solid var(--on-accent);
    border-width: 0 2px 2px 0;
    transform: translateY(-1px) rotate(45deg);
  }

  .steps li.running .st {
    border-color: var(--me);
  }

  .steps li.running .st::after {
    content: "";
    width: 8px;
    height: 8px;
    border-radius: 50%;
    background: var(--me);
  }

  .steps li.failed .st {
    border-color: var(--rec);
  }

  .steps li.skipped .st {
    border-style: dashed;
  }

  .steps li.skipped b {
    text-decoration: line-through;
  }

  .steps b {
    display: block;
    font-size: 14px;
  }

  .steps li.pending b,
  .steps li.skipped b {
    color: var(--muted);
    font-weight: 500;
  }

  .steps .d {
    font-size: 12px;
    color: var(--muted);
    font-variant-numeric: tabular-nums;
  }

  .bar {
    max-width: 380px;
    height: 6px;
    margin-top: 8px;
    border-radius: 3px;
    overflow: hidden;
    background: var(--soft);
    border: 1px solid var(--line);
  }

  .bar i {
    display: block;
    height: 100%;
    background: var(--me);
    transition: width 0.15s linear;
  }

  .ctrls {
    margin-top: 20px;
  }

  /* Empty states */
  .empty {
    display: grid;
    gap: 8px;
    justify-items: start;
    max-width: 56ch;
    margin-top: 20px;
    padding: 18px;
    border: 1px dashed var(--line);
    border-radius: 10px;
  }

  .empty p {
    margin: 0;
    font-size: 13px;
    color: var(--muted);
  }

  code {
    font: 12px var(--font-mono);
    color: var(--ink);
  }

  /* Tabs */
  .tabs {
    display: flex;
    gap: 2px;
    margin: 20px 0 18px;
    border-bottom: 1px solid var(--line);
  }

  .tabs button {
    padding: 8px 12px;
    margin-bottom: -1px;
    border: 0;
    border-bottom: 2px solid transparent;
    background: transparent;
    color: var(--muted);
    font: inherit;
    font-size: 13px;
    font-weight: 600;
    cursor: pointer;
  }

  .tabs button[aria-selected="true"] {
    color: var(--ink);
    border-bottom-color: var(--ink);
  }

  /* Summary */
  .sumdoc {
    max-width: 64ch;
    font-size: 14px;
    line-height: 1.55;
  }

  .sumdoc :global(h3) {
    margin: 20px 0 6px;
    font-size: 11px;
    font-weight: 700;
    color: var(--muted);
    text-transform: uppercase;
    letter-spacing: 0.07em;
  }

  .sumdoc :global(h3:first-child) {
    margin-top: 0;
  }

  .sumdoc :global(p) {
    margin: 0 0 8px;
  }

  .sumdoc :global(ul),
  .sumdoc :global(ol) {
    margin: 0 0 8px;
    padding-left: 18px;
  }

  .sumdoc :global(code) {
    font: 12px var(--font-mono);
  }

  /* Transcript */
  .tx {
    max-width: 72ch;
  }

  .tx-line {
    display: grid;
    grid-template-columns: 70px 48px minmax(0, 1fr);
    gap: 10px;
    padding: 7px 0;
    border-bottom: 1px solid var(--soft);
    font-size: 13px;
    line-height: 1.6;
  }

  .tx-line.echo .say {
    text-decoration: line-through;
    color: var(--muted);
  }

  .tc {
    font: 12px/1.9 var(--font-mono);
    color: var(--muted);
  }

  .who {
    font-weight: 700;
  }

  .who.me,
  td.me {
    color: var(--me);
  }

  .who.them,
  td.them {
    color: var(--them);
  }

  .tag {
    display: inline-block;
    margin-left: 6px;
    padding: 0 6px;
    border: 1px solid var(--line);
    border-radius: 4px;
    font: 500 11px/1.6 var(--font-mono);
    color: var(--muted);
  }

  .tx-mark {
    padding: 8px 0;
    border-bottom: 1px solid var(--soft);
    font: 12px var(--font-mono);
    color: var(--warn);
  }

  .tx-foot {
    margin-top: 12px;
    display: flex;
    gap: 10px;
    align-items: center;
    flex-wrap: wrap;
    font-size: 12px;
    color: var(--muted);
  }

  /* Files */
  .files-wrap {
    overflow-x: auto;
  }

  .files-note {
    margin-top: 12px;
  }

  .files {
    width: 100%;
    max-width: 760px;
    border-collapse: collapse;
    font-size: 13px;
  }

  .files td {
    padding: 8px 6px;
    border-bottom: 1px solid var(--line);
    vertical-align: top;
  }

  .files td:first-child {
    font: 12px/1.7 var(--font-mono);
    white-space: nowrap;
  }

  .files td.sz {
    text-align: right;
    color: var(--muted);
    font-variant-numeric: tabular-nums;
    white-space: nowrap;
  }

  .files td.what {
    color: var(--muted);
  }
</style>
