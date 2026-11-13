<script lang="ts">
  import { untrack } from "svelte";
  import { clock, markMoment, size, stopRecording, togglePause, type Live, type LiveStatus } from "../lib/recording";

  let { live, status, stopping }: { live: Live; status: LiveStatus | null; stopping: boolean } = $props();

  /** One bar per status update, as in the design: the newest on the right. */
  interface Bar {
    me: number;
    them: number;
    mark: boolean;
  }
  const MAX_BARS = 1500;
  const BAR_STEP = 4;
  const BAR_WIDTH = 2;

  let error = $state<string | null>(null);
  let meCanvas = $state<HTMLCanvasElement>();
  let themCanvas = $state<HTMLCanvasElement>();
  let bars: Bar[] = [];
  let marksSeen = 0;

  const paused = $derived(status?.paused ?? false);

  // Paused audio is not saved, so the waveform stops and dims instead of moving on.
  $effect(() => {
    if (!status) return;
    if (!status.paused) {
      bars.push({ me: meter(status.meLevel), them: meter(status.themLevel), mark: status.marks > marksSeen });
      if (bars.length > MAX_BARS) bars = bars.slice(-MAX_BARS);
    }
    marksSeen = status.marks;
    const dim = status.paused;
    untrack(() => draw(dim));
  });

  /** A peak from 0 to 1, as a share of the lane on a -60 to 0 dB scale. */
  function meter(peak: number): number {
    const db = 20 * Math.log10(Math.max(peak, 1e-6));
    return Math.min(1, Math.max(0, (db + 60) / 60));
  }

  function draw(dim: boolean) {
    const css = getComputedStyle(document.documentElement);
    const color = (name: string) => css.getPropertyValue(name).trim();
    if (meCanvas) drawLane(meCanvas, "me", color("--me"), color("--ink"), color("--line"), dim);
    if (themCanvas) drawLane(themCanvas, "them", color("--them"), color("--ink"), color("--line"), dim);
  }

  function drawLane(canvas: HTMLCanvasElement, key: "me" | "them", fill: string, ink: string, line: string, dim: boolean) {
    const width = canvas.clientWidth;
    const height = canvas.clientHeight;
    if (!width || !height) return;
    const ratio = window.devicePixelRatio || 1;
    if (canvas.width !== Math.round(width * ratio) || canvas.height !== Math.round(height * ratio)) {
      canvas.width = Math.round(width * ratio);
      canvas.height = Math.round(height * ratio);
    }
    const context = canvas.getContext("2d");
    if (!context) return;
    context.setTransform(ratio, 0, 0, ratio, 0, 0);
    context.clearRect(0, 0, width, height);
    context.globalAlpha = 1;
    context.fillStyle = line;
    context.fillRect(0, Math.round(height / 2), width, 1);

    const count = Math.floor(width / BAR_STEP);
    const shown = bars.slice(-count);
    const offset = count - shown.length;
    context.globalAlpha = dim ? 0.35 : 1;
    context.fillStyle = fill;
    shown.forEach((bar, i) => {
      const barHeight = Math.max(2, bar[key] * (height - 8));
      context.fillRect((offset + i) * BAR_STEP, (height - barHeight) / 2, BAR_WIDTH, barHeight);
    });
    context.globalAlpha = 1;
    context.fillStyle = ink;
    shown.forEach((bar, i) => {
      if (!bar.mark) return;
      const x = (offset + i) * BAR_STEP;
      context.fillRect(x, 0, 1.5, height);
      if (key === "me") {
        context.beginPath();
        context.moveTo(x, 0);
        context.lineTo(x + 8, 4);
        context.lineTo(x, 8);
        context.closePath();
        context.fill();
      }
    });
  }

  async function run(action: () => Promise<void>) {
    error = null;
    try {
      await action();
    } catch (e) {
      error = String(e);
    }
  }
</script>

<div class="screen">
  <div class="rec-head">
    <span class="live" class:paused={paused || stopping}>
      <i></i>{stopping ? "Saving" : paused ? "Paused" : "Recording"}
    </span>
    <span class="rec-title">{live.title}</span>
  </div>

  <div class="timer">{clock(status?.seconds ?? 0)}</div>

  <div class="lanes">
    {#if live.microphone !== null}
      <div class="lane">
        <div class="lane-lbl"><b class="me">Me</b><span>{live.microphone}</span></div>
        <canvas bind:this={meCanvas} aria-label="Microphone level"></canvas>
      </div>
    {/if}
    {#if live.systemAudio !== null}
      <div class="lane">
        <div class="lane-lbl"><b class="them">Them</b><span>{live.systemAudio}</span></div>
        <canvas bind:this={themCanvas} aria-label="System audio level"></canvas>
      </div>
    {/if}
  </div>

  <div class="facts">
    {#if live.microphone === null}
      <span><b>Microphone</b>Off</span>
    {:else if live.systemAudio === null}
      <span><b>System audio</b>Off</span>
    {/if}
    <span><b>Video</b>{live.video ?? "Off"}</span>
    <span><b>On disk</b><span class="num">{size(status?.bytes ?? 0)}</span></span>
    <span><b>Marked moments</b><span class="num">{status?.marks ?? 0}</span></span>
  </div>

  {#if error}
    <p class="error" role="alert">{error}</p>
  {/if}

  <div class="ctrls">
    <button type="button" class="btn" disabled={stopping} onclick={() => run(togglePause)}>
      {paused ? "Resume" : "Pause"}
    </button>
    <button type="button" class="btn" disabled={stopping || paused} onclick={() => run(markMoment)}>
      Mark moment <kbd>⌃</kbd><kbd>⌘</kbd><kbd>M</kbd>
    </button>
    <button type="button" class="btn danger" disabled={stopping} onclick={() => run(stopRecording)}>
      {stopping ? "Saving…" : "Stop and process"}
    </button>
  </div>
</div>

<style>
  .rec-head {
    display: flex;
    align-items: center;
    gap: 12px;
    flex-wrap: wrap;
  }

  .live {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    font-size: 12px;
    font-weight: 700;
    color: var(--rec);
    text-transform: uppercase;
    letter-spacing: 0.06em;
  }

  .live i {
    width: 8px;
    height: 8px;
    border-radius: 50%;
    background: var(--rec);
    animation: blink 1.2s infinite;
  }

  .live.paused {
    color: var(--muted);
  }

  .live.paused i {
    background: var(--muted);
    animation: none;
  }

  @keyframes blink {
    50% {
      opacity: 0.25;
    }
  }

  .rec-title {
    font-size: 16px;
    font-weight: 600;
  }

  .timer {
    margin: 16px 0 22px;
    font: 500 66px/1 var(--font-mono);
    font-variant-numeric: tabular-nums;
    letter-spacing: -0.02em;
  }

  .lanes {
    display: grid;
    gap: 10px;
    max-width: 760px;
  }

  .lane {
    display: grid;
    grid-template-columns: 160px minmax(0, 1fr);
    gap: 14px;
    align-items: center;
    padding: 10px 12px;
    border: 1px solid var(--line);
    border-radius: 10px;
  }

  .lane-lbl {
    display: grid;
    min-width: 0;
  }

  .lane-lbl b {
    font-size: 15px;
  }

  .lane-lbl span {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    font-size: 12px;
    color: var(--muted);
  }

  .me {
    color: var(--me);
  }

  .them {
    color: var(--them);
  }

  .lane canvas {
    display: block;
    width: 100%;
    height: 56px;
  }

  .facts {
    display: flex;
    gap: 22px;
    flex-wrap: wrap;
    margin-top: 16px;
    font-size: 13px;
    color: var(--muted);
  }

  .facts b {
    margin-right: 6px;
    color: var(--ink);
    font-weight: 600;
  }

  .num {
    font: 12px var(--font-mono);
    font-variant-numeric: tabular-nums;
  }

  .error {
    margin: 18px 0 0;
    max-width: 760px;
    padding: 10px 12px;
    border-radius: 8px;
    border: 1px solid color-mix(in srgb, var(--rec) 40%, var(--line));
    background: color-mix(in srgb, var(--rec) 7%, var(--surface));
    font-size: 13px;
  }

  .ctrls {
    display: flex;
    gap: 10px;
    flex-wrap: wrap;
    margin-top: 24px;
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
    opacity: 0.5;
    cursor: default;
  }

  .btn.danger {
    background: var(--rec);
    border-color: var(--rec);
    color: var(--on-accent);
  }

  .btn.danger:hover:not(:disabled) {
    background: var(--rec);
    opacity: 0.9;
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
