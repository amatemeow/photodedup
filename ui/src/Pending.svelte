<script lang="ts">
  import { onMount } from 'svelte';
  import Icon from './Icon.svelte';

  // A placeholder for something still loading that says what it waits for, and, if it is slow, why that
  // may be. Better than a blank box a person cannot tell from a broken one.
  let { what, after = 2000, height = 120 }: { what: string; after?: number; height?: number } = $props();
  let slow = $state(false);
  onMount(() => {
    const t = setTimeout(() => (slow = true), after);
    return () => clearTimeout(t);
  });
</script>

<div class="pending" role="status" style="min-height:{height}px">
  <div class="skeleton fill" aria-hidden="true"></div>
  <p class="line">Loading {what}…</p>
  {#if slow}
    <p class="hint">
      <Icon name="info" size={14} />
      <span>This is taking longer than usual. The app is probably busy, for example making previews or running a scan. It will appear on its own.</span>
    </p>
  {/if}
</div>

<style>
  .pending {
    position: relative;
    display: flex;
    flex-direction: column;
    justify-content: center;
    gap: var(--s2);
    padding: var(--s4);
    border-radius: var(--radius);
    overflow: hidden;
  }
  .fill {
    position: absolute;
    inset: 0;
    opacity: 0.6;
  }
  .line,
  .hint {
    position: relative;
    margin: 0;
  }
  .line {
    font-weight: 600;
  }
  .hint {
    display: flex;
    gap: var(--s2);
    align-items: flex-start;
    color: var(--text-secondary);
    font-size: calc(13px * var(--k));
  }
</style>
