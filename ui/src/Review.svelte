<script lang="ts">
  import { onMount } from 'svelte';
  import { Review, type Filter } from './lib/review.svelte';
  import Overview from './Overview.svelte';
  import Detail from './Detail.svelte';

  let { scanId, onExit }: { scanId: string; onExit: () => void } = $props();

  // Constructed once per mounted scan. The shell is re-created if the user
  // opens another scan, so capturing the initial id is intended.
  // svelte-ignore state_referenced_locally
  const review = new Review(scanId);

  let currentId = $state<string | null>(null); // null = overview
  let focusId = $state<string | null>(null); // overview keyboard focus
  let helpOpen = $state(false);
  let confirmAll = $state(false);

  onMount(() => {
    review.load();
  });

  const current = $derived(currentId === null ? null : (review.clusters.find((c) => c.id === currentId) ?? null));
  const ids = $derived(review.visible.map((c) => c.id));

  function open(id: string) {
    currentId = id;
    focusId = id;
  }

  function back() {
    currentId = null;
  }

  /** Moves within the current filter. If the current cluster has just left
   * the filter (accepting it under "Undecided"), `from` is its old position. */
  function step(delta: number, from?: number) {
    const pos = from ?? (currentId === null ? -1 : ids.indexOf(currentId));
    const next = ids[pos + delta];
    if (next === undefined) {
      if (delta > 0) back(); // ran off the end: review is done for this filter
      return;
    }
    open(next);
  }

  async function acceptAndNext() {
    if (!current) return;
    const pos = ids.indexOf(current.id);
    const nextId = ids[pos + 1];
    await review.accept([current]);
    if (nextId === undefined) back();
    else open(nextId);
  }

  function setFilter(f: Filter) {
    review.filter = f;
    currentId = null;
  }

  async function doAcceptAll() {
    confirmAll = false;
    await review.acceptAllUndecided();
  }

  function onKey(e: KeyboardEvent) {
    const t = e.target as HTMLElement;
    if (t.tagName === 'INPUT' || t.tagName === 'TEXTAREA' || e.metaKey || e.ctrlKey || e.altKey) return;
    if (e.key === '?') {
      helpOpen = !helpOpen;
      return;
    }
    if (helpOpen) {
      if (e.key === 'Escape') helpOpen = false;
      return;
    }
    if (e.key === 'u') {
      review.undo();
      return;
    }
    if (current) {
      // Detail-view keys that need the member list are handled in Detail.
      if (e.key === 'ArrowRight' || e.key === 'j') step(1);
      else if (e.key === 'ArrowLeft' || e.key === 'k') step(-1);
      else if (e.key === 'Enter' && t.tagName !== 'BUTTON') acceptAndNext();
      else if (e.key === 'Escape') back();
      else return;
      e.preventDefault();
    }
  }
</script>

<svelte:window onkeydown={onKey} />

<div class="shell">
  <header>
    <button class="link" onclick={onExit}>← Folders</button>
    <span class="progress">
      {review.counts.reviewed} of {review.counts.all} reviewed
      <progress max={review.counts.all || 1} value={review.counts.reviewed}></progress>
    </span>
    <button class="secondary" onclick={() => review.undo()} disabled={!review.canUndo} title="Undo (u)">Undo</button>
    <button class="secondary" onclick={() => (helpOpen = true)} title="Keyboard shortcuts (?)">?</button>
  </header>

  {#if review.error}
    <p class="error" role="alert">{review.error} <button class="link" onclick={() => (review.error = '')}>Dismiss</button></p>
  {/if}

  {#if !review.loaded}
    <p class="hint">Loading…</p>
  {:else if current}
    <Detail
      {review}
      paused={helpOpen}
      cluster={current}
      position={Math.max(ids.indexOf(current.id), 0) + 1}
      total={ids.length}
      onBack={back}
      onPrev={() => step(-1)}
      onNext={() => step(1)}
      onAccept={acceptAndNext}
    />
  {:else}
    <nav class="filters" aria-label="Filter clusters">
      {#each [['all', 'All', review.counts.all], ['undecided', 'Undecided', review.counts.undecided], ['changed', 'Changed', review.counts.changed]] as const as [key, label, n]}
        <button class="chip" class:on={review.filter === key} onclick={() => setFilter(key)}>{label} <span>{n}</span></button>
      {/each}
      <span class="spacer"></span>
      {#if confirmAll}
        <span>Accept {review.counts.undecided} suggestions?</span>
        <button onclick={doAcceptAll}>Yes, accept</button>
        <button class="secondary" onclick={() => (confirmAll = false)}>Cancel</button>
      {:else}
        <button class="secondary" disabled={review.counts.undecided === 0} onclick={() => (confirmAll = true)}>
          Accept all suggestions
        </button>
      {/if}
    </nav>
    {#if review.visible.length === 0}
      <p class="hint">Nothing here{review.filter === 'all' ? ': no duplicates were found.' : ' — all done.'}</p>
    {:else}
      <Overview {review} {focusId} onOpen={open} onFocus={(id) => (focusId = id)} />
    {/if}
  {/if}
</div>

{#if helpOpen}
  <!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
  <div class="backdrop" onclick={() => (helpOpen = false)}>
    <div class="dialog" role="dialog" tabindex="-1" aria-label="Keyboard shortcuts" onclick={(e) => e.stopPropagation()}>
      <h2>Keyboard shortcuts</h2>
      <dl>
        <dt>← → / j k</dt><dd>Previous / next cluster</dd>
        <dt>1 – 9</dt><dd>Toggle keep for that photo</dd>
        <dt>Enter</dt><dd>Accept the suggestion and go to the next</dd>
        <dt>Space</dt><dd>Large preview of the highlighted photo</dd>
        <dt>Esc</dt><dd>Close preview, back to overview</dd>
        <dt>u</dt><dd>Undo the last change</dd>
        <dt>Overview</dt><dd>Arrow keys move, Enter opens</dd>
        <dt>?</dt><dd>This list</dd>
      </dl>
      <button onclick={() => (helpOpen = false)}>Close</button>
    </div>
  </div>
{/if}

<style>
  .shell {
    max-width: 72rem;
    margin: 0 auto;
    padding: 1rem;
  }
  header {
    display: flex;
    align-items: center;
    gap: 0.75rem;
    margin-bottom: 1rem;
  }
  .progress {
    flex: 1;
    display: flex;
    align-items: center;
    gap: 0.75rem;
    color: var(--muted);
    white-space: nowrap;
  }
  .progress progress {
    flex: 1;
    max-width: 16rem;
  }
  .filters {
    display: flex;
    align-items: center;
    gap: 0.5rem;
    flex-wrap: wrap;
    margin-bottom: 1rem;
  }
  .spacer {
    flex: 1;
  }
  .chip {
    background: transparent;
    color: inherit;
    border: 1px solid var(--line);
    border-radius: 999px;
    padding: 0.25rem 0.8rem;
  }
  .chip.on {
    background: var(--accent);
    color: var(--accent-fg);
    border-color: var(--accent);
  }
  .chip span {
    opacity: 0.7;
    margin-left: 0.25rem;
  }
  .backdrop {
    position: fixed;
    inset: 0;
    background: rgb(0 0 0 / 0.5);
    display: grid;
    place-items: center;
    z-index: 10;
  }
  .dialog {
    background: var(--bg);
    border: 1px solid var(--line);
    border-radius: 12px;
    padding: 1.25rem 1.5rem;
    max-width: 28rem;
  }
  .dialog h2 {
    margin-top: 0;
    font-size: 1.1rem;
  }
  dl {
    display: grid;
    grid-template-columns: auto 1fr;
    gap: 0.35rem 1rem;
  }
  dt {
    font-family: ui-monospace, monospace;
    color: var(--muted);
  }
  dd {
    margin: 0;
  }
</style>
