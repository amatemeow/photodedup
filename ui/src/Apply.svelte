<script lang="ts">
  import { onMount } from 'svelte';
  import { api, ApiError, type ApplyResult, type Batch, type Plan, type Skipped } from './lib/api';
  import { formatBytes } from './lib/review.svelte';

  let { scanId, onClose, onChanged }: { scanId: string; onClose: () => void; onChanged: () => void } = $props();

  type Phase = 'loading' | 'preview' | 'applying' | 'done';
  let phase = $state<Phase>('loading');
  let plan = $state<Plan | null>(null);
  let result = $state<ApplyResult | null>(null);
  let batches = $state<Batch[]>([]);
  let undoing = $state<string | null>(null);
  let undoNote = $state<{ restored: number; skipped: Skipped[] } | null>(null);
  let error = $state('');

  const SHOW = 200;

  const say = (e: unknown) => (e instanceof ApiError ? e.message : e instanceof Error ? e.message : String(e));

  async function refresh() {
    try {
      batches = await api.batches();
    } catch (e) {
      error = say(e);
    }
  }

  onMount(async () => {
    try {
      plan = await api.applyPlan(scanId);
      phase = 'preview';
    } catch (e) {
      error = say(e);
      phase = 'preview';
    }
    refresh();
  });

  async function apply() {
    error = '';
    phase = 'applying';
    try {
      result = await api.apply(scanId);
      phase = 'done';
      onChanged();
    } catch (e) {
      error = say(e);
      phase = 'preview';
    }
    refresh();
  }

  async function undo(id: string) {
    error = '';
    undoing = id;
    undoNote = null;
    try {
      undoNote = await api.undoBatch(id);
      onChanged();
    } catch (e) {
      error = say(e);
    } finally {
      undoing = null;
      refresh();
    }
  }

  const when = (iso: string) => new Date(iso).toLocaleString(undefined, { dateStyle: 'medium', timeStyle: 'short' });
  const stateText: Record<Batch['state'], string> = {
    running: 'interrupted',
    done: 'in quarantine',
    partially_undone: 'partly restored',
    undone: 'restored',
  };

  // Capture phase: while this panel is open it owns the keyboard.
  function onKey(e: KeyboardEvent) {
    if (e.key === 'Escape') onClose();
    if (!(e.target as HTMLElement).closest('button, input, select')) e.stopImmediatePropagation();
  }
</script>

<svelte:window onkeydowncapture={onKey} />

<!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
<div class="backdrop" onclick={onClose}>
  <div class="panel" role="dialog" tabindex="-1" aria-label="Apply decisions" onclick={(e) => e.stopPropagation()}>
    <header>
      <h2>Apply decisions</h2>
      <button class="secondary" onclick={onClose}>Close</button>
    </header>

    <p class="safe">
      <strong>Nothing is deleted.</strong> Files you marked Quarantine are <em>moved</em> into a folder inside the scanned folder, keeping their
      layout, and every move can be undone. Files outside the scanned folder are never touched.
    </p>

    {#if error}<p class="error" role="alert">{error}</p>{/if}

    {#if phase === 'loading'}
      <p class="hint">Checking what would move…</p>
    {:else if phase === 'applying'}
      <p>Moving files…</p>
    {:else if phase === 'done' && result}
      {#if result.moved > 0}
        <p class="ok">
          Moved {result.moved} file{result.moved === 1 ? '' : 's'} ({formatBytes(result.moved_bytes)}) to quarantine.
        </p>
        <p class="hint">The review below is now out of date. Scan the folder again to refresh it. You can undo this at any time from the list below.</p>
      {:else}
        <p>Nothing was moved.</p>
      {/if}
      {#if result.skipped.length}
        <h3>Left where they were ({result.skipped.length})</h3>
        <ul class="list">
          {#each result.skipped.slice(0, SHOW) as s (s.relative_path)}<li><code>{s.relative_path}</code> <span class="hint">{s.reason}</span></li>{/each}
        </ul>
      {/if}
    {:else if plan}
      {#if plan.items.length === 0}
        <p>No file is marked for quarantine{plan.blocked.length ? ', apart from the ones listed below as refused' : ''}.</p>
      {:else}
        <p>
          <strong>{plan.items.length} file{plan.items.length === 1 ? '' : 's'}</strong> ({formatBytes(plan.total_bytes)}) would move to
          <code>{plan.root_path}/.photodedup-quarantine/…</code>
        </p>
        <ul class="list">
          {#each plan.items.slice(0, SHOW) as i (i.relative_path)}
            <li>
              <code>{i.relative_path}</code>
              <span class="hint">{formatBytes(i.size_bytes)}{i.is_sibling ? ' · companion of a marked photo' : ''}</span>
            </li>
          {/each}
          {#if plan.items.length > SHOW}<li class="hint">…and {plan.items.length - SHOW} more</li>{/if}
        </ul>
        <div class="row">
          <button onclick={apply}>Move {plan.items.length} file{plan.items.length === 1 ? '' : 's'} to quarantine</button>
          <button class="secondary" onclick={onClose}>Cancel</button>
        </div>
      {/if}
      {#if plan.blocked.length}
        <h3>Refused ({plan.blocked.length})</h3>
        <ul class="list">
          {#each plan.blocked.slice(0, SHOW) as b (b.relative_path)}<li><code>{b.relative_path}</code> <span class="hint">{b.reason}</span></li>{/each}
        </ul>
      {/if}
    {/if}

    {#if batches.length}
      <h3>Previous batches</h3>
      {#if undoNote}
        <p class="ok">
          Restored {undoNote.restored} file{undoNote.restored === 1 ? '' : 's'}.
          {#if undoNote.skipped.length}{undoNote.skipped.length} could not be restored and stay in quarantine: {undoNote.skipped[0].reason}.{/if}
        </p>
      {/if}
      <ul class="batches">
        {#each batches as b (b.id)}
          <li>
            <span>
              {when(b.created_at)} · {b.in_quarantine} file{b.in_quarantine === 1 ? '' : 's'} ({formatBytes(b.in_quarantine_bytes)}) {stateText[b.state]}
            </span>
            {#if b.in_quarantine > 0 || b.state === 'running'}
              <button class="secondary" disabled={undoing !== null} onclick={() => undo(b.id)}>
                {undoing === b.id ? 'Restoring…' : 'Undo'}
              </button>
            {/if}
          </li>
        {/each}
      </ul>
    {/if}
  </div>
</div>

<style>
  .backdrop {
    position: fixed;
    inset: 0;
    background: rgb(0 0 0 / 0.5);
    display: grid;
    place-items: center;
    z-index: 10;
    padding: 1rem;
  }
  .panel {
    background: var(--bg);
    border: 1px solid var(--line);
    border-radius: 12px;
    padding: 1.25rem 1.5rem;
    width: min(46rem, 100%);
    max-height: 88vh;
    overflow: auto;
  }
  header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    margin-bottom: 0.5rem;
  }
  h2 {
    margin: 0;
    font-size: 1.15rem;
  }
  h3 {
    font-size: 0.95rem;
    margin: 1.25rem 0 0.4rem;
  }
  .safe {
    background: color-mix(in srgb, var(--ok) 12%, transparent);
    border: 1px solid color-mix(in srgb, var(--ok) 40%, transparent);
    border-radius: 8px;
    padding: 0.6rem 0.8rem;
    margin: 0.5rem 0 1rem;
  }
  .ok {
    color: var(--ok);
  }
  .row {
    display: flex;
    gap: 0.5rem;
    margin: 0.75rem 0;
    flex-wrap: wrap;
  }
  .list {
    list-style: none;
    margin: 0.5rem 0;
    padding: 0;
    max-height: 14rem;
    overflow: auto;
    border: 1px solid var(--line);
    border-radius: 8px;
  }
  .list li {
    padding: 0.25rem 0.6rem;
    border-bottom: 1px solid var(--line);
    font-size: 0.85em;
    display: flex;
    gap: 0.6rem;
    justify-content: space-between;
  }
  .list li:last-child {
    border-bottom: 0;
  }
  code {
    word-break: break-all;
  }
  .batches {
    list-style: none;
    margin: 0;
    padding: 0;
  }
  .batches li {
    display: flex;
    justify-content: space-between;
    align-items: center;
    gap: 0.75rem;
    padding: 0.35rem 0;
    border-bottom: 1px solid var(--line);
  }
</style>
