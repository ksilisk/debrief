<script lang="ts">
  import { onMount } from "svelte";
  import { getVersion } from "@tauri-apps/api/app";

  let version = $state("");

  onMount(async () => {
    try {
      version = await getVersion();
    } catch {
      // Opened in a plain browser (npm run dev) instead of the Tauri window.
      version = "browser preview";
    }
  });
</script>

<main class="shell">
  <span class="dot" aria-hidden="true"></span>
  <h1>Debrief</h1>
  <p>App skeleton. The screens from the design arrive in M2.</p>
  {#if version}
    <p class="version">Version {version}</p>
  {/if}
</main>

<style>
  .shell {
    height: 100%;
    display: grid;
    place-content: center;
    justify-items: center;
    gap: 8px;
    padding: 24px;
    text-align: center;
  }

  .dot {
    width: 18px;
    height: 18px;
    border-radius: 50%;
    background: var(--rec);
  }

  h1 {
    margin: 0;
    font-size: 28px;
    font-weight: 700;
    letter-spacing: -0.01em;
  }

  p {
    margin: 0;
    color: var(--muted);
  }

  .version {
    font: 12px var(--font-mono);
  }
</style>
