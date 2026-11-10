<script lang="ts">
  import { onMount } from "svelte";
  import { listen } from "@tauri-apps/api/event";
  import InfoTip from "../lib/components/InfoTip.svelte";
  import { size } from "../lib/recording";
  import {
    age,
    chooseMeetingsFolder,
    editSummaryPrompt,
    loadSettings,
    providers,
    saveSettings,
    setCleanup,
    summaryModels,
    tokens,
    useSummaryModel,
    type Changes,
    type Delete,
    type ModelView,
    type Provider,
    type SettingsView,
  } from "../lib/settings";
  import { anyMissing, loadPermissions } from "../lib/permissions";
  import { meetingEvents } from "../lib/meetings";
  import { checkForUpdates, installUpdate, type UpdateStatus } from "../lib/updates";
  import {
    cancelModelDownload,
    chooseModel as useSpeechModel,
    deleteModel,
    downloadModel,
    modelNames,
    timeLeft,
    type ModelInfo,
    type SpeechModels,
  } from "../lib/models";

  let {
    onPermissions,
    update,
    speechModels,
  }: { onPermissions: () => void; update: UpdateStatus | null; speechModels: SpeechModels | null } = $props();

  const CONTEXT_CHOICES = [8_000, 32_000, 128_000];
  const CLEANUP_DAYS = [30, 90, 365];

  let view = $state<SettingsView | null>(null);
  let loadError = $state<string | null>(null);
  let recordingError = $state<string | null>(null);
  let summaryError = $state<string | null>(null);
  /** null until checked. */
  let permissionsMissing = $state<boolean | null>(null);
  let updateError = $state<string | null>(null);
  let storageError = $state<string | null>(null);

  const updateText = $derived.by(() => {
    switch (update?.state) {
      case "checking":
        return "Checking…";
      case "upToDate":
        return "Up to date";
      case "available":
        return `Version ${update.version} is ready`;
      case "downloading":
        return `Downloading the update · ${update.percent}%`;
      case "installed":
        return "Opening the new version…";
      case "failed":
        return update.message;
      default:
        return "Not checked yet";
    }
  });

  /** "5", "10" or "off". */
  let video = $state("5");
  let provider = $state<Provider | "">("");
  let keyInput = $state("");
  /** null until Test connection lists them. */
  let models = $state<ModelView[] | null>(null);
  let model = $state("");
  let contextTokens = $state(128_000);
  let choosingFolder = $state(false);
  let testing = $state(false);
  let connected = $state<string | null>(null);
  /** "never" or a number of days. */
  let cleanupAfter = $state("never");
  let cleanupDelete = $state<Delete>("recordings");
  let cleaning = $state(false);

  const saved = $derived(view?.summary ?? null);
  const host = $derived(providers.find((p) => p.id === provider)?.host ?? null);
  const savedKey = $derived(provider !== "" && saved ? saved.keys[provider] : null);
  /** The listed models, plus the saved one, which may no longer be listed. */
  const modelOptions = $derived.by(() => {
    const ids = models?.map((m) => m.id) ?? [];
    if (saved?.provider === provider && saved.model !== "" && !ids.includes(saved.model)) ids.unshift(saved.model);
    return ids;
  });
  const contextOptions = $derived([...new Set([...CONTEXT_CHOICES, contextTokens])].sort((a, b) => a - b));
  /** The choices, plus a saved number of days that isn't one of them. */
  const cleanupOptions = $derived(
    [...new Set([...CLEANUP_DAYS, view?.cleanup.afterDays ?? CLEANUP_DAYS[0]])].sort((a, b) => a - b),
  );
  const summaryStatus = $derived(
    saved?.provider ? `${providerLabel(saved.provider)} · ${saved.model}` : "Not set up yet",
  );

  onMount(() => {
    void load();
    loadPermissions()
      .then((permissions) => (permissionsMissing = anyMissing(permissions)))
      .catch(() => {});
    // Meetings can go to the Trash while Settings is open.
    const listening = listen(meetingEvents.changed, () => void loadUsage()).catch(() => undefined);
    return () => {
      void listening.then((unlisten) => unlisten?.());
    };
  });

  async function load() {
    try {
      const loaded = await loadSettings();
      view = loaded;
      video = loaded.video ? String(loaded.fps) : "off";
      provider = loaded.summary.provider ?? "";
      model = loaded.summary.model;
      contextTokens = loaded.summary.contextTokens;
      showCleanup(loaded);
    } catch (e) {
      loadError = String(e);
    }
  }

  async function loadUsage() {
    try {
      const loaded = await loadSettings();
      if (view) view.cleanup.usage = loaded.cleanup.usage;
    } catch {
      // The next change shows the problem.
    }
  }

  function showCleanup(loaded: SettingsView) {
    cleanupAfter = loaded.cleanup.afterDays === null ? "never" : String(loaded.cleanup.afterDays);
    cleanupDelete = loaded.cleanup.delete;
  }

  async function changeCleanup() {
    cleaning = true;
    storageError = null;
    try {
      await setCleanup(cleanupAfter === "never" ? null : Number(cleanupAfter), cleanupDelete);
    } catch (e) {
      storageError = String(e);
    }
    // A "no" in the confirmation keeps the saved setting; show it again.
    try {
      view = await loadSettings();
      showCleanup(view);
    } catch (e) {
      storageError = String(e);
    } finally {
      cleaning = false;
    }
  }

  function providerLabel(id: Provider): string {
    return providers.find((p) => p.id === id)?.label ?? id;
  }

  async function save(changes: Changes, onError: (message: string | null) => void) {
    try {
      await saveSettings(changes);
      onError(null);
    } catch (e) {
      onError(String(e));
    }
  }

  async function changeFolder() {
    choosingFolder = true;
    recordingError = null;
    try {
      const folder = await chooseMeetingsFolder();
      if (folder !== null && view) view.meetingsFolder = folder;
    } catch (e) {
      recordingError = String(e);
    } finally {
      choosingFolder = false;
    }
  }

  function changeVideo() {
    const on = video !== "off";
    void save(on ? { video: on, fps: Number(video) } : { video: on }, (e) => (recordingError = e));
  }

  function changeProvider() {
    models = null;
    connected = null;
    summaryError = null;
    keyInput = "";
    model = saved?.provider === provider ? (saved?.model ?? "") : "";
  }

  async function testConnection() {
    if (provider === "") return;
    const chosen = provider;
    const key = keyInput.trim();
    testing = true;
    connected = null;
    summaryError = null;
    try {
      const list = await summaryModels(chosen, key === "" ? null : key);
      if (provider !== chosen) return;
      models = list;
      connected = `Connected · ${list.length} ${list.length === 1 ? "model" : "models"}`;
      if (key !== "") {
        keyInput = "";
        view = await loadSettings();
      }
      const suggested = list.find((m) => m.suggested);
      if (model === "" && suggested) {
        model = suggested.id;
        await chooseModel();
      }
    } catch (e) {
      summaryError = String(e);
    } finally {
      testing = false;
    }
  }

  async function chooseModel() {
    if (provider === "" || model === "") return;
    const info = models?.find((m) => m.id === model);
    if (info?.contextTokens) contextTokens = info.contextTokens;
    try {
      await useSummaryModel({ provider, model, contextTokens, answerTokens: info?.answerTokens ?? null });
      view = await loadSettings();
      summaryError = null;
    } catch (e) {
      summaryError = String(e);
    }
  }

  function changeContext() {
    void save({ contextTokens }, (e) => (summaryError = e));
  }

  let modelError = $state<string | null>(null);

  function modelState(model: ModelInfo): string {
    const download = speechModels?.download;
    if (download?.model === model.name) {
      const percent = download.total > 0 ? Math.floor((download.done * 100) / download.total) : 0;
      const left = timeLeft(download.secondsLeft);
      return `Downloading · ${percent}%${left ? ` · ${left}` : ""}`;
    }
    if (model.name === speechModels?.chosen) return model.downloaded ? "In use" : "In use · not downloaded";
    if (model.downloaded) return "Downloaded";
    if (model.partialBytes > 0) return `${size(model.partialBytes)} of ${size(model.bytes)} downloaded`;
    return "Not downloaded";
  }

  async function modelAction(action: () => Promise<void>) {
    modelError = null;
    try {
      await action();
    } catch (e) {
      modelError = String(e);
    }
  }

  async function checkUpdates() {
    updateError = null;
    try {
      await checkForUpdates();
    } catch (e) {
      updateError = String(e);
    }
  }

  async function install() {
    updateError = null;
    try {
      await installUpdate();
    } catch (e) {
      updateError = String(e);
    }
  }

  async function editPrompt() {
    try {
      await editSummaryPrompt();
      summaryError = null;
    } catch (e) {
      summaryError = String(e);
    }
  }
</script>

<div class="screen">
  <h2>Settings</h2>

  {#if loadError}
    <p class="error" role="alert">{loadError}</p>
  {:else if view}
    <section class="group">
      <h3>Recording</h3>
      <div class="row">
        <span class="k"
          >Save meetings to<InfoTip
            label="Save meetings to"
            text="Any local folder. Each meeting gets its own subfolder with its audio, video and text files. Meetings saved before stay where they are."
          /></span
        >
        <span class="v">
          <span class="mono">{view.meetingsFolder}</span>
          <button type="button" class="btn small" onclick={changeFolder} disabled={choosingFolder}>Change…</button>
        </span>
      </div>
      <div class="row">
        <span class="k"
          >Start or stop recording<InfoTip
            label="Start or stop recording"
            text="A global shortcut. It works from any app, even when the Debrief window is closed."
          /></span
        >
        <span class="v"><kbd>⌃</kbd> <kbd>⌘</kbd> <kbd>R</kbd></span>
      </div>
      <div class="row">
        <span class="k"
          >macOS permissions<InfoTip
            label="macOS permissions"
            text="What macOS allows Debrief: the microphone, screen and system audio recording, and the meetings folder."
          /></span
        >
        <span class="v">
          {#if permissionsMissing !== null}
            <span class="status" class:warn={permissionsMissing}>
              {permissionsMissing ? "Some are not allowed" : "All allowed"}
            </span>
          {/if}
          <button type="button" class="btn small" onclick={onPermissions}>Review</button>
        </span>
      </div>
      <div class="row">
        <label class="k" for="video">Screen video for new recordings</label>
        <span class="v">
          <span class="select"
            ><select id="video" bind:value={video} onchange={changeVideo}>
              <option value="5">On, 5 fps</option>
              <option value="10">On, 10 fps</option>
              <option value="off">Off (audio only)</option>
            </select></span
          >
        </span>
      </div>
    </section>
    {#if recordingError}
      <p class="error" role="alert">{recordingError}</p>
    {/if}

    <section class="group">
      <h3>Transcription, on this Mac</h3>
      <div class="row top">
        <span class="k"
          >Model<InfoTip
            label="Transcription model"
            text="Speech recognition runs on this Mac; audio is never sent anywhere. Larger models are more accurate but slower. Each model is downloaded once from Hugging Face."
          /></span
        >
        <div class="models">
          {#each speechModels?.models ?? [] as model (model.name)}
            {@const busy = speechModels?.download?.model === model.name}
            <div class="model" class:chosen={model.name === speechModels?.chosen}>
              <span class="model-name">
                <b>{modelNames[model.name]?.label ?? model.name}</b>
                <span class="status">{size(model.bytes)} · {modelState(model)}</span>
                <span class="note">{modelNames[model.name]?.note ?? ""}</span>
              </span>
              <span class="model-actions">
                {#if busy}
                  <button type="button" class="btn small" onclick={() => modelAction(cancelModelDownload)}>Stop</button>
                {:else if !model.downloaded}
                  <button
                    type="button"
                    class="btn small"
                    disabled={speechModels?.download !== null}
                    onclick={() => modelAction(() => downloadModel(model.name))}
                  >
                    {model.partialBytes > 0 ? "Resume download" : "Download"}
                  </button>
                {:else if model.name !== speechModels?.chosen}
                  <button type="button" class="btn small" onclick={() => modelAction(() => useSpeechModel(model.name))}>Use</button>
                  <button type="button" class="btn small" onclick={() => modelAction(() => deleteModel(model.name))}>Delete</button>
                {/if}
              </span>
            </div>
          {/each}
          {#if modelError ?? speechModels?.error}
            <p class="model-error">{modelError ?? speechModels?.error}</p>
          {/if}
        </div>
      </div>
    </section>

    <section class="group">
      <h3>Summaries, on request</h3>
      <div class="row">
        <span class="k"
          >Summaries<InfoTip
            label="Summaries"
            text="A summary is written when you click Write summary on a meeting's Summary tab, with the prompt below or the meeting's own."
          /></span
        >
        <span class="v"><span class="status">{summaryStatus}</span></span>
      </div>
      <div class="row">
        <span class="k"
          ><label for="provider">Provider</label><InfoTip
            label="Provider"
            text="Who writes the summaries. Only the transcript text is sent; audio and video stay on this Mac."
          /></span
        >
        <span class="v">
          <span class="select"
            ><select id="provider" bind:value={provider} onchange={changeProvider}>
              {#if provider === ""}
                <option value="" disabled>Choose a provider</option>
              {/if}
              {#each providers as option (option.id)}
                <option value={option.id}>{option.label}</option>
              {/each}
            </select></span
          >
          {#if host}
            <span class="chip warn">Remote · transcripts are sent to {host}</span>
          {/if}
        </span>
      </div>
      {#if provider !== ""}
        <div class="row">
          <span class="k"
            ><label for="key">API key</label><InfoTip
              label="API key"
              text="Saved on this Mac in a file only your user can read, never in meeting files. Test connection checks it."
            /></span
          >
          <span class="v">
            <input
              class="text mono"
              id="key"
              type="password"
              bind:value={keyInput}
              placeholder={savedKey ? `Saved: ${savedKey}` : "Paste an API key"}
              autocomplete="off"
              spellcheck="false"
            />
          </span>
        </div>
        <div class="row">
          <span class="k"
            ><label for="model">Model</label><InfoTip
              label="Summary model"
              text="The model that writes summaries. Test connection lists the models your key can use."
            /></span
          >
          <span class="v">
            <span class="select"
              ><select id="model" bind:value={model} onchange={chooseModel} disabled={modelOptions.length === 0}>
                {#if model === ""}
                  <option value="" disabled>{models === null ? "Test the connection first" : "Choose a model"}</option>
                {/if}
                {#each modelOptions as id (id)}
                  <option value={id}>{id}</option>
                {/each}
              </select></span
            >
            <button type="button" class="btn small" onclick={testConnection} disabled={testing}>
              {testing ? "Testing…" : "Test connection"}
            </button>
            {#if connected}
              <span class="status ok">{connected}</span>
            {/if}
          </span>
        </div>
        <div class="row">
          <span class="k"
            ><label for="context">Model context</label><InfoTip
              label="Model context"
              text="How much text the model reads at once. Longer transcripts are summarized in parts, then joined into one summary."
            /></span
          >
          <span class="v">
            <span class="select"
              ><select id="context" bind:value={contextTokens} onchange={changeContext}>
                {#each contextOptions as option (option)}
                  <option value={option}>{tokens(option)}</option>
                {/each}
              </select></span
            >
          </span>
        </div>
      {/if}
      <div class="row">
        <span class="k"
          >Summary instructions<InfoTip
            label="Summary instructions"
            text="What the model is asked to write: a summary, decisions, action items and open questions. Edit the text to change new summaries."
          /></span
        >
        <span class="v"><button type="button" class="btn small" onclick={editPrompt}>Open in text editor</button></span>
      </div>
    </section>
    {#if summaryError}
      <p class="error" role="alert">{summaryError}</p>
    {/if}

    <section class="group">
      <h3>Storage</h3>
      <div class="row">
        <span class="k"
          ><label for="cleanup-delete">What to delete</label><InfoTip
            label="What to delete"
            text="Audio and video: the recordings go, the transcript and summary stay. Whole meeting: the meeting folder goes, transcript and summary included."
          /></span
        >
        <span class="v">
          <span class="select"
            ><select
              id="cleanup-delete"
              bind:value={cleanupDelete}
              onchange={changeCleanup}
              disabled={cleaning}
            >
              <option value="recordings">Audio and video</option>
              <option value="meeting">Whole meeting</option>
            </select></span
          >
        </span>
      </div>
      <div class="row">
        <span class="k"
          ><label for="cleanup-after">Delete after</label><InfoTip
            label="Delete after"
            text="Meetings older than this go to the Trash, counted from the day they were recorded. Debrief checks at launch and every 12 hours. A meeting that isn’t transcribed yet is never deleted."
          /></span
        >
        <span class="v">
          <span class="select"
            ><select id="cleanup-after" bind:value={cleanupAfter} onchange={changeCleanup} disabled={cleaning}>
              <option value="never">Never</option>
              {#each cleanupOptions as days (days)}
                <option value={String(days)}>{age(days)}</option>
              {/each}
            </select></span
          >
          <span class="status">
            {view.cleanup.usage.meetings === 1 ? "1 meeting takes" : `${view.cleanup.usage.meetings} meetings take`}
            {size(view.cleanup.usage.bytes)}
          </span>
        </span>
      </div>
    </section>
    {#if storageError}
      <p class="error" role="alert">{storageError}</p>
    {/if}

    <section class="group">
      <h3>Updates</h3>
      <div class="row">
        <span class="k"
          >Debrief {update?.current ?? ""}<InfoTip
            label="Updates"
            text="Debrief checks GitHub for a new version at launch and every 12 hours. It installs only when you click, never during a recording or processing."
          /></span
        >
        <span class="v">
          <span class="status" class:ok={update?.state === "upToDate"} class:warn={update?.state === "failed"}>{updateText}</span>
          {#if update?.state === "available"}
            <button type="button" class="btn small primary" onclick={install}>Install and restart</button>
          {:else if update?.state !== "downloading" && update?.state !== "installed"}
            <button type="button" class="btn small" onclick={checkUpdates} disabled={update?.state === "checking"}>
              Check for updates
            </button>
          {/if}
        </span>
      </div>
    </section>
    {#if updateError}
      <p class="error" role="alert">{updateError}</p>
    {/if}
  {/if}
</div>

<style>
  .screen {
    max-width: 760px;
  }

  h2 {
    margin: 0;
    font-size: 22px;
    font-weight: 700;
    letter-spacing: -0.01em;
  }

  .group {
    margin-top: 16px;
    border: 1px solid var(--line);
    border-radius: 10px;
  }

  h3 {
    margin: 0;
    padding: 9px 14px;
    border-radius: 9px 9px 0 0;
    border-bottom: 1px solid var(--line);
    background: var(--soft);
    font-size: 11px;
    font-weight: 700;
    color: var(--muted);
    text-transform: uppercase;
    letter-spacing: 0.07em;
  }

  .row {
    display: grid;
    grid-template-columns: 190px minmax(0, 1fr);
    gap: 14px;
    align-items: center;
    padding: 10px 14px;
  }

  .row + .row {
    border-top: 1px solid var(--line);
  }

  .k {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    flex-wrap: wrap;
    font-size: 13px;
    font-weight: 600;
  }

  .v {
    display: flex;
    gap: 8px;
    align-items: center;
    flex-wrap: wrap;
    min-width: 0;
    font-size: 13px;
  }

  .row.top {
    align-items: start;
  }

  .models {
    display: grid;
    gap: 2px;
    min-width: 0;
  }

  .model {
    display: flex;
    gap: 10px;
    align-items: center;
    justify-content: space-between;
    padding: 6px 8px;
    border-radius: 7px;
    font-size: 13px;
  }

  .model.chosen {
    background: var(--soft);
  }

  .model-name {
    display: grid;
    gap: 1px;
    min-width: 0;
  }

  .note {
    font-size: 12px;
    color: var(--muted);
  }

  .model-actions {
    display: flex;
    gap: 6px;
    flex: none;
  }

  .model-error {
    margin: 4px 8px 0;
    font-size: 12px;
    color: var(--rec);
  }

  .mono {
    font-family: var(--font-mono);
    font-size: 12px;
  }

  .status {
    font-size: 12px;
    color: var(--muted);
  }

  .status.ok {
    color: var(--ok);
  }

  .status.warn {
    color: var(--warn);
  }

  .chip {
    padding: 2px 8px;
    border-radius: 999px;
    border: 1px solid;
    font-size: 11px;
    font-weight: 600;
    white-space: nowrap;
  }

  .chip.warn {
    color: var(--warn);
    border-color: color-mix(in srgb, var(--warn) 45%, transparent);
  }

  kbd {
    padding: 0 5px;
    border: 1px solid var(--line);
    border-bottom-width: 2px;
    border-radius: 4px;
    background: var(--surface);
    color: var(--ink);
    font: 500 11px var(--font-mono);
  }

  .text {
    flex: 1 1 220px;
    min-width: 0;
    padding: 5px 8px;
    border: 1px solid var(--line);
    border-radius: 7px;
    background: var(--surface);
    color: var(--ink);
  }

  .select {
    position: relative;
    display: inline-flex;
    max-width: 300px;
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
    max-width: 300px;
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

  .btn:hover:not(:disabled) {
    background: var(--soft);
  }

  .btn:disabled {
    opacity: 0.55;
    cursor: default;
  }

  .btn.primary {
    background: var(--ink);
    border-color: var(--ink);
    color: var(--surface);
  }

  .btn.primary:hover:not(:disabled) {
    opacity: 0.9;
    background: var(--ink);
  }

  .btn.small {
    padding: 4px 10px;
    font-size: 12px;
  }

  .error {
    margin: 10px 0 0;
    padding: 10px 12px;
    border-radius: 8px;
    border: 1px solid color-mix(in srgb, var(--rec) 40%, var(--line));
    background: color-mix(in srgb, var(--rec) 7%, var(--surface));
    font-size: 13px;
  }

  @media (max-width: 600px) {
    .row {
      grid-template-columns: minmax(0, 1fr);
      gap: 6px;
    }
  }
</style>
