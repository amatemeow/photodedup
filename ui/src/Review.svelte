<script lang="ts">
  import { onMount } from 'svelte';
  import { Review, tabInfo, tabs, type Filter, type Sort, type Tab } from './lib/review.svelte';
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
  let plan = $state<{ groups: number; quarantine: number } | null>(null); // pending "accept all" confirmation

  const undoKey = /Mac|iPhone|iPad/i.test(navigator.userAgent) ? '⌘Z' : 'Ctrl+Z';

  onMount(() => {
    review.load();
  });

  const current = $derived(currentId === null ? null : (review.groups.find((g) => g.id === currentId) ?? null));
  const ids = $derived(review.visible.map((g) => g.id));

  function open(id: string) {
    currentId = id;
    focusId = id;
  }

  function back() {
    currentId = null;
  }

  function setTab(t: Tab) {
    review.tab = t;
    review.filter = 'all';
    currentId = null;
    focusId = null;
    plan = null;
  }

  function step(delta: number) {
    const pos = currentId === null ? -1 : ids.indexOf(currentId);
    const next = ids[pos + delta];
    if (next === undefined) {
      if (delta > 0) back(); // ran off the end: this filter is done
      return;
    }
    open(next);
  }

  async function acceptAndNext() {
    if (!current) return;
    const nextId = ids[ids.indexOf(current.id) + 1];
    await review.accept([current]);
    if (nextId === undefined) back();
    else open(nextId);
  }

  function setFilter(f: Filter) {
    review.filter = f;
    currentId = null;
  }

  function askAcceptAll() {
    const p = review.undecidedPlan();
    plan = { groups: p.groups.length, quarantine: p.quarantine };
  }

  async function doAcceptAll() {
    plan = null;
    await review.acceptAllUndecided();
  }

  function onKey(e: KeyboardEvent) {
    const t = e.target as HTMLElement;
    if (t.tagName === 'INPUT' || t.tagName === 'TEXTAREA') return;
    // Undo is the platform's own chord: Cmd+Z on a Mac, Ctrl+Z elsewhere.
    if ((e.metaKey || e.ctrlKey) && !e.shiftKey && !e.altKey && e.key.toLowerCase() === 'z') {
      if (!helpOpen) review.undo();
      e.preventDefault();
      return;
    }
    if (e.metaKey || e.ctrlKey || e.altKey) return;
    if (e.key === '?') {
      helpOpen = !helpOpen;
      return;
    }
    if (helpOpen) {
      if (e.key === 'Escape') helpOpen = false;
      return;
    }
    if (current) {
      // Keys that need the member list (1-9, Space) are handled in Detail.
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
    <button class="secondary" onclick={() => review.undo()} disabled={!review.canUndo} title="Undo ({undoKey})">Undo</button>
    <button class="secondary" onclick={() => (helpOpen = true)} title="Keyboard shortcuts (?)">?</button>
  </header>

  {#if review.error}
    <p class="error" role="alert">{review.error} <button class="link" onclick={() => (review.error = '')}>Dismiss</button></p>
  {/if}

  {#if !review.loaded}
    <p class="hint">Loading…</p>
  {:else}
    <div class="tabs" role="tablist">
      {#each tabs as t (t)}
        <button role="tab" aria-selected={review.tab === t} class="tab" class:on={review.tab === t} onclick={() => setTab(t)} title={tabInfo[t].blurb}>
          {tabInfo[t].title} <span>{review.tabCount(t)}</span>
        </button>
      {/each}
    </div>

    {#if current}
      <Detail
        {review}
        paused={helpOpen}
        group={current}
        position={Math.max(ids.indexOf(current.id), 0) + 1}
        total={ids.length}
        onBack={back}
        onPrev={() => step(-1)}
        onNext={() => step(1)}
        onAccept={acceptAndNext}
      />
    {:else}
      <p class="blurb">{tabInfo[review.tab].blurb}</p>

      <nav class="filters" aria-label="Filter">
        {#each [['all', 'All', review.counts.all], ['undecided', 'Undecided', review.counts.undecided], ['changed', 'Changed', review.counts.changed]] as const as [key, label, n]}
          <button class="chip" class:on={review.filter === key} onclick={() => setFilter(key)}>{label} <span>{n}</span></button>
        {/each}
        <span class="spacer"></span>
        <label class="sort">
          Sort
          <select value={review.sort} onchange={(e) => review.setSort(e.currentTarget.value as Sort)}>
            <option value="oldest">Oldest first</option>
            <option value="newest">Newest first</option>
          </select>
        </label>
        {#if plan}
          <span>
            Accept {plan.groups} suggestion{plan.groups === 1 ? '' : 's'}?
            {plan.quarantine} file{plan.quarantine === 1 ? '' : 's'} would be marked for quarantine (nothing moves until you apply).
          </span>
          <button onclick={doAcceptAll}>Yes, accept</button>
          <button class="secondary" onclick={() => (plan = null)}>Cancel</button>
        {:else}
          <button class="secondary" disabled={review.counts.undecided === 0} onclick={askAcceptAll}>Accept all suggestions</button>
        {/if}
      </nav>
      {#if review.visible.length === 0}
        <p class="hint">{review.counts.all === 0 ? 'Nothing in this category.' : 'Nothing here — all done.'}</p>
      {:else}
        <Overview {review} {focusId} onOpen={open} onFocus={(id) => (focusId = id)} />
      {/if}
    {/if}
  {/if}
</div>

{#if helpOpen}
  <!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
  <div class="backdrop" onclick={() => (helpOpen = false)}>
    <div class="dialog" role="dialog" tabindex="-1" aria-label="Keyboard shortcuts" onclick={(e) => e.stopPropagation()}>
      <h2>Keyboard shortcuts</h2>
      <dl>
        <dt>{undoKey}</dt><dd>Undo the last change</dd>
        <dt>← → / j k</dt><dd>Previous / next</dd>
        <dt>1 – 9</dt><dd>Toggle keep for that photo</dd>
        <dt>Enter</dt><dd>Accept the suggestion and go to the next</dd>
        <dt>Space</dt><dd>Large preview of the highlighted photo</dd>
        <dt>Esc</dt><dd>Close preview, back to overview</dd>
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
  .tabs {
    display: flex;
    gap: 0.25rem;
    flex-wrap: wrap;
    border-bottom: 1px solid var(--line);
    margin-bottom: 0.75rem;
  }
  .tab {
    background: transparent;
    color: var(--muted);
    border: 0;
    border-bottom: 2px solid transparent;
    border-radius: 0;
    padding: 0.5rem 0.9rem;
  }
  .tab.on {
    color: var(--fg);
    border-bottom-color: var(--accent);
  }
  .tab span {
    opacity: 0.7;
    margin-left: 0.25rem;
  }
  .blurb {
    color: var(--muted);
    margin: 0 0 1rem;
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
  .sort {
    display: flex;
    align-items: center;
    gap: 0.4rem;
    color: var(--muted);
  }
  .sort select {
    font: inherit;
    padding: 0.25rem 0.4rem;
    border: 1px solid var(--line);
    border-radius: 8px;
    background: transparent;
    color: var(--fg);
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
