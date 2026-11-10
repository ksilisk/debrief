<script lang="ts">
  // Shown while the chosen Whisper model isn't downloaded: a button to get it,
  // then its progress. Nothing is shown once it is downloaded.
  import { size } from "../recording";
  import {
    cancelModelDownload,
    chosenModel,
    downloadModel,
    modelLabel,
    timeLeft,
    type SpeechModels,
  } from "../models";

  let { models, onSettings }: { models: SpeechModels | null; onSettings?: () => void } = $props();

  let error = $state<string | null>(null);

  const chosen = $derived(models ? chosenModel(models) : undefined);
  const download = $derived(models?.download ?? null);
  const shown = $derived(chosen !== undefined && (!chosen.downloaded || download !== null));
  const percent = $derived(download && download.total > 0 ? Math.floor((download.done * 100) / download.total) : 0);
  const failure = $derived(error ?? models?.error ?? null);

  async function start() {
    if (!chosen) return;
    error = null;
    try {
      await downloadModel(chosen.name);
    } catch (e) {
      error = String(e);
    }
  }

  async function stop() {
    try {
      await cancelModelDownload();
    } catch (e) {
      error = String(e);
    }
  }
</script>

{#if shown && chosen}
  <div class="callout" class:busy={download !== null} role="status">
    {#if download}
      <div class="text">
        <b>Downloading {modelLabel(download.model)}</b>
        <span class="detail">
          {percent}% · {size(download.done)} of {size(download.total)}{download.secondsLeft !== null
            ? ` · ${timeLeft(download.secondsLeft)}`
            : ""}
        </span>
      </div>
      <button type="button" class="btn" onclick={stop}>Stop</button>
      <div class="bar" aria-hidden="true"><i style:width="{percent}%"></i></div>
    {:else}
      <div class="text">
        <b><span class="dot"></span>The speech model isn't downloaded</b>
        <span class="detail">
          Recordings are saved, but can't be transcribed until it is.
          {#if onSettings}
            <button type="button" class="link" onclick={onSettings}>Choose a smaller model</button>
          {/if}
        </span>
        {#if failure}
          <span class="failure">{failure}</span>
        {/if}
      </div>
      <button type="button" class="btn" onclick={start}>
        {chosen.partialBytes > 0
          ? `Resume · ${size(chosen.bytes - chosen.partialBytes)} left`
          : `Download · ${size(chosen.bytes)}`}
      </button>
    {/if}
  </div>
{/if}

<style>
  .callout {
    display: grid;
    grid-template-columns: minmax(0, 1fr) auto;
    gap: 10px 16px;
    align-items: center;
    margin: 14px 0 0;
    padding: 12px 14px;
    border-radius: 10px;
    border: 1px solid color-mix(in srgb, var(--warn) 45%, var(--line));
    background: color-mix(in srgb, var(--warn) 8%, var(--surface));
  }

  .callout.busy {
    border-color: var(--line);
    background: var(--soft);
  }

  .text {
    display: grid;
    gap: 2px;
    min-width: 0;
  }

  b {
    display: flex;
    align-items: center;
    gap: 8px;
    font-size: 13px;
  }

  .detail {
    font-size: 12px;
    color: var(--muted);
  }

  .busy .detail {
    font-variant-numeric: tabular-nums;
  }

  .dot {
    width: 7px;
    height: 7px;
    flex: none;
    border-radius: 50%;
    background: var(--warn);
  }

  .callout:not(.busy) .detail,
  .failure {
    padding-left: 15px;
  }

  .bar {
    grid-column: 1 / -1;
    height: 6px;
    border-radius: 3px;
    overflow: hidden;
    background: var(--surface);
    border: 1px solid var(--line);
  }

  .bar i {
    display: block;
    height: 100%;
    background: var(--me);
    transition: width 0.2s linear;
  }

  .btn {
    padding: 6px 12px;
    border-radius: 8px;
    border: 1px solid var(--line);
    background: var(--surface);
    color: var(--ink);
    font: inherit;
    font-size: 13px;
    font-weight: 600;
    white-space: nowrap;
    cursor: pointer;
  }

  .btn:hover {
    background: var(--soft);
  }

  .link {
    padding: 0;
    border: 0;
    background: none;
    color: var(--ink);
    font: inherit;
    text-decoration: underline;
    text-decoration-color: var(--line);
    text-underline-offset: 2px;
    cursor: pointer;
  }

  .link:hover {
    text-decoration-color: currentColor;
  }

  .failure {
    font-size: 12px;
    color: var(--rec);
  }
</style>
