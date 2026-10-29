<script lang="ts">
  import { onMount } from 'svelte';

  let { folder, onConfirm, onCancel }: { folder: string; onConfirm: () => void; onCancel: () => void } = $props();

  // Focus starts on Cancel, so a stray Enter or Space does the safe thing.
  let cancelButton: HTMLButtonElement;
  onMount(() => cancelButton.focus());

  function onKey(e: KeyboardEvent) {
    if (e.key === 'Escape') {
      e.stopPropagation();
      onCancel();
    }
  }
</script>

<svelte:window onkeydown={onKey} />

<!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
<div class="scrim" onclick={onCancel}>
  <div class="sheet" role="alertdialog" aria-modal="true" aria-labelledby="rescan-title" aria-describedby="rescan-text" tabindex="-1" onclick={(e) => e.stopPropagation()}>
    <h2 id="rescan-title" class="lead">Scan “{folder}” again?</h2>
    <p id="rescan-text" class="secondary text">
      It takes a while on a big folder. The new scan replaces the current one; your keep and quarantine choices on files that haven't
      changed are kept. Nothing in the folder is touched.
    </p>
    <div class="actions">
      <button class="btn" bind:this={cancelButton} onclick={onCancel}>Cancel</button>
      <button class="btn primary" onclick={onConfirm}>Scan again</button>
    </div>
  </div>
</div>

<style>
  .text {
    margin: var(--s2) 0 var(--s4);
  }
  .actions {
    display: flex;
    justify-content: flex-end;
    gap: var(--s2);
  }
</style>
