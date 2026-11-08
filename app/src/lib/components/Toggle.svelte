<script lang="ts">
  let {
    checked = $bindable(false),
    label,
    disabled = false,
  }: { checked?: boolean; label: string; disabled?: boolean } = $props();
</script>

<span class="switch">
  <input type="checkbox" role="switch" aria-label={label} bind:checked {disabled} />
  <span class="track"></span>
</span>

<style>
  .switch {
    position: relative;
    display: inline-flex;
    width: 36px;
    height: 22px;
    flex: none;
  }

  input {
    position: absolute;
    inset: 0;
    width: 100%;
    height: 100%;
    margin: 0;
    opacity: 0;
    cursor: pointer;
    z-index: 1;
  }

  input:disabled {
    cursor: default;
  }

  .track {
    position: relative;
    width: 36px;
    height: 22px;
    border-radius: 999px;
    background: var(--line);
    transition: background 0.15s;
  }

  .track::after {
    content: "";
    position: absolute;
    top: 2px;
    left: 2px;
    width: 18px;
    height: 18px;
    border-radius: 50%;
    background: var(--surface);
    box-shadow: 0 1px 2px color-mix(in srgb, var(--ink) 25%, transparent);
    transition: transform 0.15s;
  }

  input:checked + .track {
    background: var(--ink);
  }

  input:checked + .track::after {
    transform: translateX(14px);
  }

  input:disabled + .track {
    opacity: 0.45;
  }

  input:focus-visible + .track {
    outline: 2px solid var(--me);
    outline-offset: 2px;
  }
</style>
