<script lang="ts">
  import { onMount } from "svelte";
  import InfoTip from "../lib/components/InfoTip.svelte";
  import {
    isMissing,
    loadPermissions,
    meetingsFolderAccess,
    needsRestart,
    openPrivacySettings,
    relaunch,
    requestPermission,
    type Folder,
    type Kind,
    type Permissions,
    type Status,
  } from "../lib/permissions";

  let { onDone }: { onDone: () => void } = $props();

  /** How often the statuses are read again while this screen is open, in milliseconds. */
  const REFRESH = 2000;

  const rows: { kind: Exclude<Kind, "files">; label: string; tip: string }[] = [
    {
      kind: "microphone",
      label: "Microphone",
      tip: "Records your voice as the Me track.",
    },
    {
      kind: "screenRecording",
      label: "Screen & System Audio Recording",
      tip: "Debrief records through macOS screen capture, also for audio only, and records the screen when video is on. After you turn it on, quit and reopen Debrief.",
    },
    {
      kind: "systemAudio",
      label: "System Audio Recording",
      tip: "Records the sound of the call as the Them track. Without it, Them stays silent. In System Settings it is in the System Audio Recording Only list.",
    },
  ];

  let permissions = $state<Permissions | null>(null);
  let folder = $state<Folder | null>(null);
  let waiting = $state<Kind | null>(null);
  let restarting = $state(false);
  let error = $state<string | null>(null);

  const allAllowed = $derived(
    permissions !== null &&
      folder !== null &&
      !Object.values(permissions).some(isMissing) &&
      !isMissing(folder.status),
  );
  const restart = $derived(permissions !== null && needsRestart(permissions));

  onMount(() => {
    void refresh();
    void checkFolder();
    const timer = setInterval(() => void refresh(), REFRESH);
    const onFocus = () => {
      void refresh();
      void checkFolder();
    };
    window.addEventListener("focus", onFocus);
    return () => {
      clearInterval(timer);
      window.removeEventListener("focus", onFocus);
    };
  });

  async function refresh() {
    try {
      permissions = await loadPermissions();
    } catch (e) {
      error = String(e);
    }
  }

  async function checkFolder() {
    try {
      folder = await meetingsFolderAccess();
    } catch (e) {
      error = String(e);
    }
  }

  function statusOf(kind: Kind): Status | null {
    if (kind === "files") return folder?.status ?? null;
    return permissions?.[kind] ?? null;
  }

  async function allow(kind: Kind) {
    waiting = kind;
    error = null;
    try {
      const status = await requestPermission(kind);
      if (kind === "files" && folder) folder.status = status;
      await refresh();
    } catch (e) {
      error = String(e);
    } finally {
      waiting = null;
    }
  }

  async function openSettings(kind: Kind) {
    error = null;
    try {
      await openPrivacySettings(kind);
    } catch (e) {
      error = String(e);
    }
  }

  async function quitAndReopen() {
    restarting = true;
    error = null;
    try {
      await relaunch();
    } catch (e) {
      error = String(e);
      restarting = false;
    }
  }
</script>

{#snippet status(kind: Kind)}
  {@const value = statusOf(kind)}
  {#if value === null}
    <span class="state">Checking…</span>
  {:else if waiting === kind}
    <span class="state">Waiting for your answer…</span>
  {:else}
    <span class="state">
      <span class="dot {value === 'allowed' ? 'ok' : value === 'unknown' ? 'off' : 'warn'}"></span>
      {value === "allowed"
        ? "Allowed"
        : value === "notAsked"
          ? "Not asked yet"
          : value === "denied"
            ? "Turned off"
            : "Can't check"}
    </span>
    {#if value === "notAsked"}
      <button type="button" class="btn small" onclick={() => allow(kind)} disabled={waiting !== null}>Allow…</button>
    {:else if value !== "allowed"}
      <button type="button" class="btn small" onclick={() => openSettings(kind)}>Open System Settings</button>
    {/if}
  {/if}
{/snippet}

<div class="screen">
  <h2>Permissions</h2>

  <section class="group">
    <h3>What macOS allows Debrief</h3>
    {#each rows as row (row.kind)}
      <div class="row">
        <span class="k">{row.label}<InfoTip label={row.label} text={row.tip} /></span>
        <span class="v">{@render status(row.kind)}</span>
      </div>
    {/each}
    <div class="row">
      <span class="k"
        >Meetings folder<InfoTip
          label="Meetings folder"
          text="Debrief saves meetings in this folder. macOS asks once before an app can use Documents, Desktop or Downloads."
        /></span
      >
      <span class="v">
        {#if folder?.path}<span class="mono">{folder.path}</span>{/if}
        {@render status("files")}
      </span>
    </div>
  </section>

  {#if restart}
    <div class="note">
      <span>Screen recording is on. It starts working after Debrief quits and opens again.</span>
      <button type="button" class="btn small primary" onclick={quitAndReopen} disabled={restarting}>
        {restarting ? "Reopening…" : "Quit and reopen Debrief"}
      </button>
    </div>
  {/if}

  {#if error}
    <p class="error" role="alert">{error}</p>
  {/if}

  <div class="actions">
    <button type="button" class="btn" class:primary={allAllowed} onclick={onDone}>
      {allAllowed ? "Done" : "Skip for now"}
    </button>
  </div>
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
    grid-template-columns: 250px minmax(0, 1fr);
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
    gap: 10px;
    align-items: center;
    flex-wrap: wrap;
    min-width: 0;
    font-size: 13px;
  }

  .mono {
    font-family: var(--font-mono);
    font-size: 12px;
  }

  .state {
    display: inline-flex;
    align-items: center;
    color: var(--muted);
  }

  .dot {
    display: inline-block;
    width: 7px;
    height: 7px;
    margin-right: 6px;
    border-radius: 50%;
  }

  .dot.ok {
    background: var(--ok);
  }

  .dot.warn {
    background: var(--warn);
  }

  .dot.off {
    background: var(--line);
  }

  .note {
    display: flex;
    gap: 12px;
    align-items: center;
    flex-wrap: wrap;
    margin: 12px 0 0;
    padding: 10px 12px;
    border-radius: 8px;
    border: 1px solid color-mix(in srgb, var(--warn) 45%, var(--line));
    background: color-mix(in srgb, var(--warn) 8%, var(--surface));
    font-size: 13px;
  }

  .error {
    margin: 12px 0 0;
    padding: 10px 12px;
    border-radius: 8px;
    border: 1px solid color-mix(in srgb, var(--rec) 40%, var(--line));
    background: color-mix(in srgb, var(--rec) 7%, var(--surface));
    font-size: 13px;
  }

  .actions {
    margin-top: 18px;
  }

  .btn {
    display: inline-flex;
    align-items: center;
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

  @media (max-width: 600px) {
    .row {
      grid-template-columns: minmax(0, 1fr);
      gap: 6px;
    }
  }
</style>
