<script lang="ts">
  import { untrack } from "svelte";

  let {
    value,
    size,
    placeholder,
    onSave,
    onCancel,
  }: {
    /** The title as typed; empty when the meeting uses the date and time. */
    value: string;
    size: "heading" | "item";
    placeholder: string;
    /** Gets the trimmed title, after Enter or a click elsewhere in the window. */
    onSave: (title: string) => void;
    onCancel: () => void;
  } = $props();

  let text = $state(untrack(() => value));
  let done = false;

  function finish(save: boolean) {
    if (done) return;
    done = true;
    if (save) onSave(text.trim());
    else onCancel();
  }

  function key(event: KeyboardEvent) {
    if (event.isComposing) return;
    if (event.key === "Enter") finish(true);
    if (event.key === "Escape") finish(false);
  }

  // Switching to another app blurs the field too; the edit waits for the window to come back.
  function blur() {
    if (document.hasFocus()) finish(true);
  }

  function focusAll(input: HTMLInputElement) {
    input.focus();
    input.select();
  }
</script>

<input
  class={size}
  type="text"
  aria-label="Meeting title"
  {placeholder}
  autocomplete="off"
  bind:value={text}
  onkeydown={key}
  onblur={blur}
  use:focusAll
/>

<style>
  /* The underline is a shadow, so the field is as tall as the title it replaces. */
  input {
    display: block;
    width: 100%;
    padding: 0;
    border: 0;
    border-radius: 0;
    box-shadow: 0 1px 0 var(--me);
    background: transparent;
    color: var(--ink);
    font: inherit;
    outline: none;
  }

  .heading {
    font-size: 22px;
    font-weight: 700;
    letter-spacing: -0.01em;
  }

  .item {
    font-size: 13px;
    font-weight: 600;
  }
</style>
