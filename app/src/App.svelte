<script lang="ts">
  import { onMount } from "svelte";
  import { listen } from "@tauri-apps/api/event";
  import { Menu } from "@tauri-apps/api/menu";
  import { screenTitles, type Screen } from "./lib/screens";
  import { clock, currentRecording, events, showInFinder, type Live, type LiveStatus, type Saved } from "./lib/recording";
  import {
    dayLabel,
    duration,
    listMeetings,
    meetingEvents,
    moveToTrash,
    processingProgress,
    statusLabels,
    timeOfDay,
    type Finished,
    type MeetingItem,
    type Progress,
  } from "./lib/meetings";
  import NewRecording from "./screens/NewRecording.svelte";
  import Recording from "./screens/Recording.svelte";
  import Meeting from "./screens/Meeting.svelte";
  import Settings from "./screens/Settings.svelte";
  import Permissions from "./screens/Permissions.svelte";
  import { anyMissing, isMissing, loadPermissions, meetingsFolderAccess } from "./lib/permissions";
  import { installUpdate, updateEvent, updateStatus, type UpdateStatus } from "./lib/updates";
  import { loadSpeechModels, speechModelsEvent, type SpeechModels } from "./lib/models";

  let screen = $state<Screen>("new");
  let live = $state<Live | null>(null);
  let status = $state<LiveStatus | null>(null);
  let stopping = $state(false);
  let error = $state<string | null>(null);
  let meetings = $state<MeetingItem[]>([]);
  let selected = $state<string | null>(null);
  let progress = $state<Progress | null>(null);
  /** Goes up whenever meetings change, so an open meeting loads again. */
  let version = $state(0);
  let models = $state<SpeechModels | null>(null);
  let update = $state<UpdateStatus | null>(null);
  let updateError = $state<string | null>(null);
  /** An update can't be installed during a recording or processing. */
  const updateBlocked = $derived(live !== null || progress !== null);
  /** What went wrong while saving a recording, by meeting folder. */
  let saveProblems = $state<Record<string, string[]>>({});

  /** New recording and Recording share one place: a running recording decides which one shows. */
  const shown = $derived<Screen>(screen === "new" || screen === "recording" ? (live ? "recording" : "new") : screen);

  // Model files can change outside Debrief, so each screen starts from a fresh look.
  $effect(() => {
    void shown;
    loadSpeechModels()
      .then((current) => (models = current))
      .catch(() => {});
  });

  /** The list without the live recording, which has its own item, grouped by day. */
  const groups = $derived.by(() => {
    const result: { label: string; items: MeetingItem[] }[] = [];
    for (const item of meetings) {
      if (item.status === "recording") continue;
      const label = dayLabel(item.startedAt);
      const group = result.at(-1);
      if (group?.label === label) group.items.push(item);
      else result.push({ label, items: [item] });
    }
    return result;
  });

  function openMeeting(folder: string) {
    selected = folder;
    screen = "meeting";
  }

  async function refresh() {
    try {
      const before = meetings;
      meetings = await listMeetings();
      // The open meeting went to the Trash.
      const listed = (list: MeetingItem[]) => list.some((item) => item.folder === selected);
      if (selected && listed(before) && !listed(meetings)) {
        selected = null;
        if (screen === "meeting") screen = "new";
      }
    } catch {
      // Opened in a plain browser (npm run dev) instead of the Tauri window.
    }
    version += 1;
  }

  async function meetingMenu(event: MouseEvent, item: MeetingItem) {
    event.preventDefault();
    const busy = item.status === "processing" || item.status === "waiting";
    try {
      const menu = await Menu.new({
        items: [
          { text: "Show in Finder", action: () => void showInFinder(item.folder).catch(() => {}) },
          { item: "Separator" },
          { text: "Move to Trash", enabled: !busy, action: () => void moveToTrash(item.folder).catch(() => {}) },
        ],
      });
      await menu.popup();
    } catch {
      // Opened in a plain browser (npm run dev) instead of the Tauri window.
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

  /** At launch, a missing permission opens the Permissions screen, unless something else is already showing. */
  async function checkPermissions() {
    const showIfMissing = (missing: boolean) => {
      if (missing && screen === "new" && !live) screen = "permissions";
    };
    try {
      showIfMissing(anyMissing(await loadPermissions()));
      showIfMissing(isMissing((await meetingsFolderAccess()).status));
    } catch {
      // Opened in a plain browser (npm run dev) instead of the Tauri window.
    }
  }

  onMount(() => {
    const stops = [
      // The menu bar icon and the global shortcut ask for a screen through this event.
      listen<Screen>("navigate", (event) => (screen = event.payload)),
      listen<string>(meetingEvents.open, (event) => openMeeting(event.payload)),
      listen<Live>(events.started, (event) => {
        live = event.payload;
        status = null;
        stopping = false;
        error = null;
        screen = "recording";
      }),
      listen<LiveStatus>(events.status, (event) => (status = event.payload)),
      listen(events.stopping, () => (stopping = true)),
      listen<Saved>(events.stopped, (event) => {
        live = null;
        status = null;
        stopping = false;
        if (event.payload.problems.length > 0) saveProblems[event.payload.folder] = event.payload.problems;
        openMeeting(event.payload.folder);
      }),
      listen<string>(events.failed, (event) => {
        error = event.payload;
        screen = "new";
      }),
      listen(meetingEvents.changed, () => void refresh()),
      listen<UpdateStatus>(updateEvent, (event) => (update = event.payload)),
      listen<SpeechModels>(speechModelsEvent, (event) => (models = event.payload)),
      listen<Progress>(meetingEvents.progress, (event) => {
        const stepChanged = JSON.stringify(event.payload.steps) !== JSON.stringify(progress?.steps);
        progress = event.payload;
        if (stepChanged) version += 1;
      }),
      listen<Finished>(meetingEvents.finished, () => {
        progress = null;
        void refresh();
      }),
    ].map((listening) => listening.catch(() => undefined));

    currentRecording()
      .then((current) => {
        if (current) live = current;
      })
      .catch(() => {});
    processingProgress()
      .then((current) => (progress = current))
      .catch(() => {});
    void refresh();
    void checkPermissions();
    updateStatus()
      .then((current) => (update = current))
      .catch(() => {});
    loadSpeechModels()
      .then((current) => (models = current))
      .catch(() => {});

    return () => {
      for (const listening of stops) void listening.then((unlisten) => unlisten?.());
    };
  });
</script>

<div class="window">
  <header class="titlebar" data-tauri-drag-region>
    <span class="title" data-tauri-drag-region>Debrief</span>
    <button
      type="button"
      class="icon-btn"
      aria-label="Settings"
      aria-current={shown === "settings"}
      onclick={() => (screen = "settings")}
    >
      <svg
        viewBox="0 0 24 24"
        width="17"
        height="17"
        fill="none"
        stroke="currentColor"
        stroke-width="1.8"
        stroke-linecap="round"
        stroke-linejoin="round"
        aria-hidden="true"
      >
        <circle cx="12" cy="12" r="3" />
        <path
          d="M19.4 15a1.7 1.7 0 0 0 .3 1.8l.1.1a2 2 0 1 1-2.8 2.8l-.1-.1a1.7 1.7 0 0 0-1.8-.3 1.7 1.7 0 0 0-1 1.5V21a2 2 0 1 1-4 0v-.1a1.7 1.7 0 0 0-1.1-1.5 1.7 1.7 0 0 0-1.8.3l-.1.1a2 2 0 1 1-2.8-2.8l.1-.1a1.7 1.7 0 0 0 .3-1.8 1.7 1.7 0 0 0-1.5-1H3a2 2 0 1 1 0-4h.1a1.7 1.7 0 0 0 1.5-1.1 1.7 1.7 0 0 0-.3-1.8l-.1-.1a2 2 0 1 1 2.8-2.8l.1.1a1.7 1.7 0 0 0 1.8.3H9a1.7 1.7 0 0 0 1-1.5V3a2 2 0 1 1 4 0v.1a1.7 1.7 0 0 0 1 1.5 1.7 1.7 0 0 0 1.8-.3l.1-.1a2 2 0 1 1 2.8 2.8l-.1.1a1.7 1.7 0 0 0-.3 1.8V9a1.7 1.7 0 0 0 1.5 1H21a2 2 0 1 1 0 4h-.1a1.7 1.7 0 0 0-1.5 1z"
        />
      </svg>
    </button>
  </header>

  <div class="body">
    <aside class="side" aria-label="Meetings">
      <button
        type="button"
        class="new-btn"
        aria-current={shown === "new"}
        onclick={() => (screen = "new")}
      >
        <svg
          viewBox="0 0 24 24"
          width="15"
          height="15"
          fill="none"
          stroke="currentColor"
          stroke-width="2"
          stroke-linecap="round"
          aria-hidden="true"
        >
          <path d="M12 5v14M5 12h14" />
        </svg>
        New recording
      </button>
      {#if live}
        <button type="button" class="live-item" aria-current={shown === "recording"} onclick={() => (screen = "recording")}>
          <b>{live.title}</b>
          <span><i></i>{stopping ? "Saving" : status?.paused ? "Paused" : "Recording"} · {clock(status?.seconds ?? 0)}</span>
        </button>
      {/if}
      {#each groups as group (group.label)}
        <div class="group">{group.label}</div>
        <ul class="mlist">
          {#each group.items as item (item.folder)}
            <li>
              <button
                type="button"
                class="mitem"
                aria-current={shown === "meeting" && selected === item.folder}
                onclick={() => openMeeting(item.folder)}
                oncontextmenu={(event) => void meetingMenu(event, item)}
              >
                <b>{item.title}</b>
                <span>
                  <span class="num">{timeOfDay(item.startedAt)} · {duration(item.durationSeconds)}</span>
                  <span class="chip {item.status}">
                    {item.status === "processing" && progress?.folder === item.folder ? `${progress.percent}%` : statusLabels[item.status]}
                  </span>
                </span>
              </button>
            </li>
          {/each}
        </ul>
      {/each}
      {#if !live && groups.length === 0}
        <p class="empty">No meetings yet.</p>
      {/if}
      {#if update?.state === "available" || update?.state === "downloading" || update?.state === "installed"}
        <div class="update-card" role="status">
          {#if update.state === "available"}
            <b>Debrief {update.version} is ready</b>
            {#if updateBlocked}
              <span>You can install it once the {live ? "recording" : "processing"} is done.</span>
            {/if}
            <button type="button" class="update-btn" onclick={install} disabled={updateBlocked}>Install and restart</button>
          {:else if update.state === "downloading"}
            <b>Downloading the update</b>
            <span class="num">{update.percent}%</span>
          {:else}
            <b>Opening the new version…</b>
          {/if}
          {#if updateError}
            <span class="update-error">{updateError}</span>
          {/if}
        </div>
      {/if}
    </aside>

    <main class="pane">
      {#if shown === "new"}
        <NewRecording bind:error onPermissions={() => (screen = "permissions")} {models} onSettings={() => (screen = "settings")} />
      {:else if shown === "recording" && live}
        <Recording {live} {status} {stopping} />
      {:else if shown === "meeting" && selected}
        <Meeting
          folder={selected}
          progress={progress?.folder === selected ? progress : null}
          {version}
          problems={saveProblems[selected] ?? []}
          {models}
          onSettings={() => (screen = "settings")}
        />
      {:else if shown === "settings"}
        <Settings onPermissions={() => (screen = "permissions")} {update} speechModels={models} />
      {:else if shown === "permissions"}
        <Permissions onDone={() => (screen = "new")} />
      {:else}
        <h2>{screenTitles[shown]}</h2>
        <p class="sub">This screen is not built yet.</p>
      {/if}
    </main>
  </div>
</div>

<style>
  .window {
    height: 100vh;
    display: grid;
    grid-template-rows: 40px minmax(0, 1fr);
  }

  /* Drawn under macOS's own traffic lights (titleBarStyle "Overlay" in tauri.conf.json). */
  .titlebar {
    position: relative;
    display: flex;
    align-items: center;
    padding: 0 12px;
    background: var(--chrome);
    border-bottom: 1px solid var(--line);
    user-select: none;
    -webkit-user-select: none;
  }

  .title {
    position: absolute;
    left: 50%;
    transform: translateX(-50%);
    font-size: 13px;
    font-weight: 600;
    color: var(--muted);
  }

  .icon-btn {
    margin-left: auto;
    width: 30px;
    height: 30px;
    display: grid;
    place-items: center;
    border: 0;
    border-radius: 7px;
    background: transparent;
    color: var(--muted);
    cursor: pointer;
  }

  .icon-btn:hover,
  .icon-btn[aria-current="true"] {
    background: var(--soft);
    color: var(--ink);
  }

  .body {
    display: grid;
    grid-template-columns: 236px minmax(0, 1fr);
    min-height: 0;
  }

  .side {
    display: flex;
    flex-direction: column;
    gap: 2px;
    padding: 14px 10px;
    background: var(--side);
    border-right: 1px solid var(--line);
    overflow-y: auto;
    /* The list loads after the window; it should not move what is in view. */
    overflow-anchor: none;
  }

  .new-btn {
    display: flex;
    align-items: center;
    gap: 8px;
    margin-bottom: 8px;
    padding: 8px 10px;
    border: 1px solid var(--line);
    border-radius: 8px;
    background: var(--surface);
    color: var(--ink);
    font: inherit;
    font-size: 13px;
    font-weight: 600;
    cursor: pointer;
  }

  .new-btn[aria-current="true"] {
    border-color: var(--ink);
  }

  .live-item {
    display: grid;
    gap: 2px;
    padding: 8px 10px;
    border: 0;
    border-radius: 8px;
    background: color-mix(in srgb, var(--rec) 9%, transparent);
    color: var(--ink);
    font: inherit;
    text-align: left;
    cursor: pointer;
  }

  .live-item b {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    font-size: 13px;
  }

  .live-item span {
    display: flex;
    align-items: center;
    gap: 6px;
    font-size: 12px;
    color: var(--rec);
    font-variant-numeric: tabular-nums;
  }

  .live-item i {
    width: 7px;
    height: 7px;
    border-radius: 50%;
    background: var(--rec);
  }

  .group {
    padding: 12px 10px 4px;
    font-size: 11px;
    font-weight: 600;
    color: var(--muted);
    text-transform: uppercase;
    letter-spacing: 0.06em;
  }

  .mlist {
    list-style: none;
    margin: 0;
    padding: 0;
    display: grid;
    gap: 2px;
  }

  .mitem {
    width: 100%;
    display: grid;
    gap: 2px;
    padding: 8px 10px;
    border: 0;
    border-radius: 8px;
    background: transparent;
    color: var(--ink);
    font: inherit;
    text-align: left;
    cursor: pointer;
  }

  .mitem:hover {
    background: var(--soft);
  }

  .mitem[aria-current="true"] {
    background: var(--surface);
    box-shadow: 0 0 0 1px var(--line);
  }

  .mitem b {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    font-size: 13px;
    font-weight: 600;
  }

  .mitem > span {
    display: flex;
    gap: 6px;
    align-items: center;
    flex-wrap: wrap;
    font-size: 12px;
    color: var(--muted);
  }

  .num {
    font-variant-numeric: tabular-nums;
  }

  .chip {
    padding: 0 7px;
    border: 1px solid var(--line);
    border-radius: 999px;
    font-size: 11px;
    line-height: 1.5;
    color: var(--muted);
  }

  .chip.ready {
    color: var(--ok);
    border-color: color-mix(in srgb, var(--ok) 40%, transparent);
  }

  .chip.processing {
    color: var(--me);
    border-color: color-mix(in srgb, var(--me) 40%, transparent);
    font-variant-numeric: tabular-nums;
  }

  .chip.failed,
  .chip.cutOff {
    color: var(--warn);
    border-color: color-mix(in srgb, var(--warn) 45%, transparent);
  }

  .empty {
    margin: 4px 10px;
    font-size: 12px;
    color: var(--muted);
  }

  .update-card {
    position: sticky;
    bottom: 0;
    display: grid;
    gap: 4px;
    margin-top: auto;
    padding: 10px;
    border: 1px solid var(--line);
    border-radius: 8px;
    background: var(--surface);
    box-shadow: var(--shadow);
    font-size: 12px;
  }

  .update-card b {
    font-size: 13px;
  }

  .update-card span {
    color: var(--muted);
  }

  .update-card .update-error {
    color: var(--rec);
  }

  .update-btn {
    justify-self: start;
    margin-top: 4px;
    padding: 4px 10px;
    border-radius: 7px;
    border: 1px solid var(--ink);
    background: var(--ink);
    color: var(--surface);
    font: inherit;
    font-size: 12px;
    font-weight: 600;
    cursor: pointer;
  }

  .update-btn:disabled {
    opacity: 0.45;
    cursor: default;
  }

  .pane {
    min-width: 0;
    padding: 28px 32px 36px;
    overflow-y: auto;
  }

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
</style>
