<script lang="ts">
  // An (i) button that shows its text on hover or keyboard focus.
  let { label, text }: { label: string; text: string } = $props();
  const id = `tip-${Math.random().toString(36).slice(2)}`;
</script>

<span class="wrap">
  <button type="button" class="tip" aria-label={`About: ${label}`} aria-describedby={id}>i</button>
  <span class="pop" role="tooltip" {id}>{text}</span>
</span>

<style>
  .wrap {
    position: relative;
    display: inline-flex;
  }

  .tip {
    width: 16px;
    height: 16px;
    padding: 0;
    display: grid;
    place-items: center;
    border-radius: 50%;
    border: 1px solid var(--line);
    background: var(--soft);
    color: var(--muted);
    font: italic 700 10px/1 var(--font-ui);
    cursor: help;
  }

  .tip:hover,
  .tip:focus-visible {
    color: var(--ink);
    border-color: var(--muted);
  }

  .pop {
    position: absolute;
    left: -8px;
    bottom: calc(100% + 8px);
    z-index: 25;
    width: max-content;
    max-width: 280px;
    padding: 8px 10px;
    border-radius: 8px;
    background: var(--ink);
    color: var(--surface);
    box-shadow: var(--shadow);
    font: 400 12px/1.45 var(--font-ui);
    white-space: normal;
    opacity: 0;
    visibility: hidden;
    transform: translateY(4px);
    pointer-events: none;
    transition:
      opacity 0.12s,
      transform 0.12s,
      visibility 0.12s;
  }

  .wrap:hover .pop,
  .tip:focus + .pop {
    opacity: 1;
    visibility: visible;
    transform: none;
  }
</style>
