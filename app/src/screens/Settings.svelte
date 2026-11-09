<script lang="ts">
  import { onMount } from "svelte";
  import InfoTip from "../lib/components/InfoTip.svelte";
  import Toggle from "../lib/components/Toggle.svelte";
  import { size } from "../lib/recording";
  import {
    chooseMeetingsFolder,
    countHintTokens,
    editSummaryPrompt,
    loadSettings,
    providers,
    saveSettings,
    summaryModels,
    tokens,
    useSummaryModel,
    type Changes,
    type ModelView,
    type Provider,
    type SettingsView,
  } from "../lib/settings";
  import { anyMissing, loadPermissions } from "../lib/permissions";
  import { checkForUpdates, installUpdate, type UpdateStatus } from "../lib/updates";

  let { onPermissions, update }: { onPermissions: () => void; update: UpdateStatus | null } = $props();

  const CONTEXT_CHOICES = [8_000, 32_000, 128_000];
  /** Names and terms are saved and counted once typing pauses this long, in milliseconds. */
  const TYPING_PAUSE = 300;

  let view = $state<SettingsView | null>(null);
  let loadError = $state<string | null>(null);
  let recordingError = $state<string | null>(null);
  let hintsError = $state<string | null>(null);
  let summaryError = $state<string | null>(null);
  /** null until checked. */
  let permissionsMissing = $state<boolean | null>(null);
  let updateError = $state<string | null>(null);

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
  let hints = $state("");
  let hintTokens = $state<number | null>(null);
  let summariesOn = $state(false);
  let provider = $state<Provider | "">("");
  let keyInput = $state("");
  /** null until Test connection lists them. */
  let models = $state<ModelView[] | null>(null);
  let model = $state("");
  let contextTokens = $state(128_000);
  let choosingFolder = $state(false);
  let testing = $state(false);
  let connected = $state<string | null>(null);

  let hintTimer: ReturnType<typeof setTimeout> | undefined;

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
  const maxHints = $derived(view?.maxHintTokens ?? 0);
  const overLimit = $derived(hintTokens !== null && hintTokens > maxHints);
  const summaryStatus = $derived(
    saved?.provider ? `${providerLabel(saved.provider)} · ${saved.model}` : "Not set up yet",
  );

  onMount(() => {
    void load();
    loadPermissions()
      .then((permissions) => (permissionsMissing = anyMissing(permissions)))
      .catch(() => {});
    return () => {
      if (hintTimer !== undefined) {
        clearTimeout(hintTimer);
        void saveHints();
      }
    };
  });

  async function load() {
    try {
      const loaded = await loadSettings();
      view = loaded;
      video = loaded.video ? String(loaded.fps) : "off";
      hints = loaded.hints;
      hintTokens = loaded.hintTokens;
      summariesOn = loaded.summary.onByDefault;
      provider = loaded.summary.provider ?? "";
      model = loaded.summary.model;
      contextTokens = loaded.summary.contextTokens;
    } catch (e) {
      loadError = String(e);
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

  function typedHints() {
    clearTimeout(hintTimer);
    hintTimer = setTimeout(() => void saveHints(), TYPING_PAUSE);
  }

  async function saveHints() {
    hintTimer = undefined;
    const text = hints;
    try {
      await saveSettings({ hints: text });
      const count = await countHintTokens(text);
      if (text === hints) hintTokens = count;
      hintsError = null;
    } catch (e) {
      hintsError = String(e);
    }
  }

  function changeSummariesOn(on: boolean) {
    summariesOn = on;
    void save({ summariesOn: on }, (e) => (summaryError = e));
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
      <div class="row">
        <span class="k"
          >Model<InfoTip
            label="Transcription model"
            text="Speech recognition runs on this Mac with this model. Audio is never sent anywhere."
          /></span
        >
        <span class="v">
          Whisper {view.whisper.name}
          <span class="status">
            {view.whisper.downloaded
              ? `${size(view.whisper.bytes)} · downloaded`
              : "Not downloaded. Run debrief model download in Terminal."}
          </span>
        </span>
      </div>
      <div class="row">
        <span class="k"
          ><label for="hints">Names and terms</label><InfoTip
            label="Names and terms"
            text={`Spelling hints for Whisper: names, products and jargon it might otherwise misspell. Put each on its own line or separate them with commas. Everything said is still transcribed; this list only helps spelling. Whisper uses only the last ${maxHints} tokens.`}
          /></span
        >
        <span class="v">
          <textarea id="hints" bind:value={hints} oninput={typedHints} spellcheck="false" placeholder="Grafana, Kubernetes, Telemost"></textarea>
          <span class="tokens" class:over={overLimit} aria-live="polite">
            {#if hintTokens === null}
              The token count appears once the Whisper model is downloaded.
            {:else}
              {hintTokens} / {maxHints} tokens{overLimit ? " · too long: Whisper drops the words at the start" : ""}
            {/if}
          </span>
        </span>
      </div>
    </section>
    {#if hintsError}
      <p class="error" role="alert">{hintsError}</p>
    {/if}

    <section class="group">
      <h3>Summaries (optional)</h3>
      <div class="row">
        <span class="k"
          >Write summaries<InfoTip
            label="Write summaries"
            text="Default for new recordings. You can switch it for one recording on the New recording screen, or write a summary later from the meeting screen."
          /></span
        >
        <span class="v">
          <Toggle
            bind:checked={() => summariesOn, changeSummariesOn}
            label="Write summaries for new recordings"
          />
          <span class="status">{summaryStatus}</span>
        </span>
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

  textarea {
    width: 100%;
    min-height: 54px;
    padding: 6px 8px;
    resize: vertical;
    border: 1px solid var(--line);
    border-radius: 7px;
    background: var(--surface);
    color: var(--ink);
    font: inherit;
    font-size: 13px;
  }

  .tokens {
    flex-basis: 100%;
    font: 12px var(--font-mono);
    color: var(--muted);
  }

  .tokens.over {
    color: var(--rec);
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
