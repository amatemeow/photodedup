<script lang="ts">
  import { onMount } from 'svelte';
  import { api, ApiError, type ApplyResult, type Batch, type Plan, type Skipped } from './lib/api';
  import { formatBytes } from './lib/review.svelte';
  import Icon from './Icon.svelte';

  let { scanId, onClose, onChanged }: { scanId: string; onClose: () => void; onChanged: () => void } = $props();

  type Phase = 'loading' | 'preview' | 'applying' | 'done';
  let phase = $state<Phase>('loading');
  let plan = $state<Plan | null>(null);
  const videoCount = $derived(plan?.items.filter((i) => i.media_class === 'Video').length ?? 0);
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
    } catch (e) {
      error = say(e);
    }
    phase = 'preview';
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
    running: 'Interrupted',
    done: 'In quarantine',
    partially_undone: 'Partly restored',
    undone: 'Restored',
  };
  /** `a/b/c.jpg` -> folder `a/b/`, name `c.jpg`: the name is what you recognise. */
  const split = (p: string) => {
    const i = p.lastIndexOf('/');
    return i < 0 ? { dir: '', name: p } : { dir: p.slice(0, i + 1), name: p.slice(i + 1) };
  };
  /** Shorten a long absolute path from the middle, keeping both ends. */
  const shorten = (p: string, max = 54) => (p.length <= max ? p : `${p.slice(0, 18)}…${p.slice(-(max - 19))}`);

  // Capture phase: while this panel is open it owns the keyboard.
  function onKey(e: KeyboardEvent) {
    if (e.key === 'Escape') onClose();
    if (!(e.target as HTMLElement).closest('button, input, select')) e.stopImmediatePropagation();
  }
</script>

<svelte:window onkeydowncapture={onKey} />

<!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
<div class="scrim" onclick={onClose}>
  <div class="sheet wide" role="dialog" aria-modal="true" aria-labelledby="apply-title" tabindex="-1" onclick={(e) => e.stopPropagation()}>
    <header>
      <h2 id="apply-title" class="title">Apply decisions</h2>
      <button class="btn icon plain" onclick={onClose} aria-label="Close"><Icon name="x" /></button>
    </header>

    <div class="notice success safe">
      <Icon name="check" size={18} />
      <p>
        <strong>Nothing is deleted.</strong> Files you marked Quarantine are <em>moved</em> into a folder inside the scanned folder, keeping their
        layout, and every move can be undone. Nothing outside the scanned folder is ever touched.
      </p>
    </div>

    {#if error}<div class="notice danger" role="alert"><Icon name="info" size={18} /><p>{error}</p></div>{/if}

    {#if phase === 'loading'}
      <div class="skeleton" style="height:120px"></div>
    {:else if phase === 'applying'}
      <p class="lead row"><span class="spinner"></span> Moving files…</p>
    {:else if phase === 'done' && result}
      {#if result.moved > 0}
        <div class="notice success">
          <Icon name="check" size={18} />
          <p><strong>Moved {result.moved} file{result.moved === 1 ? '' : 's'}</strong> ({formatBytes(result.moved_bytes)}) to quarantine. The review is now out of date: scan the folder again to refresh it. You can undo this from the list below.</p>
        </div>
      {:else}
        <p class="secondary">Nothing was moved.</p>
      {/if}
      {#if result.skipped.length}
        <h3 class="lead">Left where they were <span class="hint">{result.skipped.length}</span></h3>
        <ul class="list">
          {#each result.skipped.slice(0, SHOW) as s (s.relative_path)}<li><code>{s.relative_path}</code><span class="hint">{s.reason}</span></li>{/each}
        </ul>
      {/if}
    {:else if plan}
      {#if plan.items.length === 0}
        <p class="secondary">No file is marked for quarantine{plan.blocked.length ? ', apart from the ones listed below as refused' : ''}.</p>
      {:else}
        {#if plan.emptied_groups > 0}
          <p class="notice warning" role="status">
            <Icon name="info" size={16} />
            <span><strong>{plan.emptied_groups} group{plan.emptied_groups === 1 ? '' : 's'} will keep nothing.</strong> Every file in {plan.emptied_groups === 1 ? 'it is' : 'them are'} marked for quarantine. Nothing is deleted, and undoing this puts them back.</span>
          </p>
        {/if}
        <div class="stats">
          <div><span class="big">{plan.items.length}</span><span class="hint">file{plan.items.length === 1 ? '' : 's'}</span></div>
          <div>
            <span class="big">{plan.items.length - videoCount}<span class="sep"> / </span>{videoCount}</span>
            <span class="hint">photos / videos</span>
          </div>
          <div><span class="big">{formatBytes(plan.total_bytes)}</span><span class="hint">to be moved</span></div>
          <div class="where"><span class="hint">Destination</span><code title="{plan.root_path}/.photodedup-quarantine/">{shorten(plan.root_path)}/.photodedup-quarantine/…</code></div>
        </div>
        <ul class="list">
          {#each [...plan.items].sort((a, b) => Number(b.media_class === 'Video') - Number(a.media_class === 'Video')).slice(0, SHOW) as i (i.relative_path)}
            {@const p = split(i.relative_path)}
            <li>
              <span class="file"><span class="dir">{p.dir}</span><span class="nm">{p.name}</span>{#if i.media_class === 'Video'}<span class="badge">video</span>{/if}{#if i.is_sibling}<span class="badge">companion</span>{/if}</span>
              <span class="hint">{formatBytes(i.size_bytes)}</span>
            </li>
          {/each}
          {#if plan.items.length > SHOW}<li class="hint">…and {plan.items.length - SHOW} more</li>{/if}
        </ul>
      {/if}
      {#if plan.blocked.length}
        <h3 class="lead">Refused <span class="hint">{plan.blocked.length}</span></h3>
        <ul class="list">
          {#each plan.blocked.slice(0, SHOW) as b (b.relative_path)}<li><code>{b.relative_path}</code><span class="hint">{b.reason}</span></li>{/each}
        </ul>
      {/if}
    {/if}

    {#if batches.length}
      <h3 class="lead">Previous batches</h3>
      {#if undoNote}
        <div class="notice success">
          <Icon name="check" size={18} />
          <p>
            Restored {undoNote.restored} file{undoNote.restored === 1 ? '' : 's'}.
            {#if undoNote.skipped.length}{undoNote.skipped.length} could not be restored and stay in quarantine: {undoNote.skipped[0].reason}.{/if}
          </p>
        </div>
      {/if}
      <ul class="batches">
        {#each batches as b (b.id)}
          <li>
            <span class="what">
              <strong>{when(b.created_at)}</strong>
              <span class="hint">{b.in_quarantine} file{b.in_quarantine === 1 ? '' : 's'} · {formatBytes(b.in_quarantine_bytes)} · {stateText[b.state]}</span>
            </span>
            {#if b.in_quarantine > 0 || b.state === 'running'}
              <button class="btn" disabled={undoing !== null} onclick={() => undo(b.id)}>
                {#if undoing === b.id}<span class="spinner"></span> Restoring…{:else}<Icon name="undo" /> Undo{/if}
              </button>
            {/if}
          </li>
        {/each}
      </ul>
    {/if}

    {#if phase === 'preview' && plan && plan.items.length > 0}
      <footer>
        <button class="btn" onclick={onClose}>Cancel</button>
        <button class="btn primary" onclick={apply}>Move {plan.items.length} file{plan.items.length === 1 ? '' : 's'} to quarantine</button>
      </footer>
    {/if}
  </div>
</div>

<style>
  .sheet {
    display: grid;
    gap: var(--s4);
    align-content: start;
  }
  header {
    display: flex;
    align-items: center;
    justify-content: space-between;
  }
  .row {
    display: flex;
    align-items: center;
    gap: var(--s3);
  }
  .stats {
    display: grid;
    grid-template-columns: auto auto 1fr;
    gap: var(--s5);
    align-items: end;
    padding: var(--s3) var(--s4);
    border-radius: var(--radius);
    background: var(--surface-2);
  }
  .stats > div {
    display: grid;
    gap: 2px;
    min-width: 0;
  }
  .big {
    font-size: var(--fs-title);
    font-weight: 650;
    font-variant-numeric: tabular-nums;
    letter-spacing: -0.02em;
  }
  .where code {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .list {
    list-style: none;
    margin: 0;
    padding: 0;
    max-height: 260px;
    overflow: auto;
    border-radius: var(--radius);
    box-shadow: inset 0 0 0 1px var(--separator);
  }
  .list li {
    display: flex;
    justify-content: space-between;
    gap: var(--s3);
    padding: 7px var(--s3);
    border-bottom: 1px solid var(--separator);
  }
  .list li:last-child {
    border-bottom: 0;
  }
  .file {
    display: flex;
    align-items: center;
    gap: var(--s2);
    min-width: 0;
    overflow: hidden;
    white-space: nowrap;
  }
  .dir {
    color: var(--label-3);
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .nm {
    font-weight: 600;
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
    gap: var(--s3);
    padding: var(--s2) 0;
    border-bottom: 1px solid var(--separator);
  }
  .what {
    display: grid;
    gap: 1px;
  }
  footer {
    display: flex;
    justify-content: flex-end;
    gap: var(--s2);
    position: sticky;
    bottom: calc(var(--s5) * -1);
    margin: 0 calc(var(--s5) * -1) calc(var(--s5) * -1);
    padding: var(--s3) var(--s5);
    background: color-mix(in srgb, var(--surface) 92%, transparent);
    backdrop-filter: blur(10px);
    border-top: 1px solid var(--separator);
  }
  @media (max-width: 560px) {
    .stats {
      grid-template-columns: 1fr 1fr;
    }
    .where {
      grid-column: 1 / -1;
    }
  }
</style>
