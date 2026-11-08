<script lang="ts">
  import { onMount } from "svelte";
  import { listen, type UnlistenFn } from "@tauri-apps/api/event";
  import { screenTitles, type Screen } from "./lib/screens";

  let screen = $state<Screen>("new");

  onMount(() => {
    let unlisten: UnlistenFn | undefined;
    // The menu bar icon and the global shortcut ask for a screen through this event.
    listen<Screen>("navigate", (event) => {
      screen = event.payload;
    })
      .then((stop) => (unlisten = stop))
      .catch(() => {
        // Opened in a plain browser (npm run dev) instead of the Tauri window.
      });
    return () => unlisten?.();
  });
</script>

<div class="window">
  <header class="titlebar" data-tauri-drag-region>
    <span class="title" data-tauri-drag-region>Debrief</span>
    <button
      type="button"
      class="icon-btn"
      aria-label="Settings"
      aria-current={screen === "settings"}
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
        aria-current={screen === "new"}
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
      <p class="empty">No meetings yet.</p>
    </aside>

    <main class="pane">
      <h2>{screenTitles[screen]}</h2>
      <p class="sub">This screen is not built yet.</p>
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

  .empty {
    margin: 4px 10px;
    font-size: 12px;
    color: var(--muted);
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
