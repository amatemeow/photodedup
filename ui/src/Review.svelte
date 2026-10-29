<script lang="ts">
  import { onMount } from 'svelte';
  import { Review, tabInfo, tabs, type Filter, type Sort, type Tab } from './lib/review.svelte';
  import { api, type ScanInfo, type Settings } from './lib/api';
  import { changedSince, usedLine } from './lib/knobs';
  import Overview from './Overview.svelte';
  import Detail from './Detail.svelte';
  import Apply from './Apply.svelte';
  import Tuning from './Tuning.svelte';
  import Icon, { type Name } from './Icon.svelte';

  let {
    scanId,
    onExit,
    onRescan,
  }: { scanId: string; onExit: () => void; onRescan: (root: string, includeVideos: boolean) => void } = $props();

  // Constructed once per mounted scan. The shell is re-created if the user
  // opens another scan, so capturing the initial id is intended.
  // svelte-ignore state_referenced_locally
  const review = new Review(scanId);

  let currentId = $state<string | null>(null); // null = overview
  let focusId = $state<string | null>(null); // overview keyboard focus
  let helpOpen = $state(false);
  let applyOpen = $state(false);
  let tuningOpen = $state(false);
  // Set once files have actually moved: the results below no longer match disk.
  let moved = $state(false);
  let plan = $state<{ groups: number; quarantine: number } | null>(null); // pending "accept all" confirmation

  const undoKey = /Mac|iPhone|iPad/i.test(navigator.userAgent) ? '⌘Z' : 'Ctrl+Z';

  // What this scan was made with, and what is saved now: when they differ
  // the results are out of date, and the banner says how.
  let info = $state<ScanInfo | null>(null);
  let saved = $state<Settings | null>(null);
  const changes = $derived(changedSince(info?.used ?? null, saved));
  const folderName = $derived(info ? (info.root_path.split(/[\\/]/).filter(Boolean).pop() ?? info.root_path) : '');

  const loadInfo = () =>
    Promise.all([api.scanInfo(scanId), api.settings()])
      .then(([i, s]) => {
        info = i;
        saved = s;
      })
      .catch(() => {}); // the banner is a convenience; the review works without it

  onMount(() => {
    review.load();
    loadInfo();
  });

  // The matching sliders regrouped the scan: show the new lists, keep the view.
  async function tuned() {
    await Promise.all([review.load(true), loadInfo()]);
  }

  const current = $derived(currentId === null ? null : (review.groups.find((g) => g.id === currentId) ?? null));
  const ids = $derived(review.visible.map((g) => g.id));
  // The photos of the next two groups, so the detail view can fetch them ahead.
  const upcoming = $derived.by(() => {
    if (!current) return [];
    const at = ids.indexOf(current.id);
    return ids
      .slice(at + 1, at + 3)
      .flatMap((id) => review.groups.find((g) => g.id === id)?.members ?? [])
      .slice(0, 8);
  });

  const icons: Record<Tab, Name> = { exact: 'layers', true: 'compare', burst: 'burst', similar: 'photo', orphans: 'film' };
  const overall = $derived.by(() => {
    let all = 0;
    let reviewed = 0;
    for (const t of tabs) {
      all += review.countsByTab[t].all;
      reviewed += review.countsByTab[t].reviewed;
    }
    return { all, reviewed };
  });

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
    if (applyOpen) return; // the apply panel owns the keyboard while it is open
    const t = e.target as HTMLElement;
    if (t.tagName === 'INPUT' || t.tagName === 'TEXTAREA' || t.tagName === 'SELECT') return;
    // Undo is the platform's own chord: Cmd+Z on a Mac, Ctrl+Z elsewhere.
    if ((e.metaKey || e.ctrlKey) && !e.shiftKey && !e.altKey && e.key.toLowerCase() === 'z') {
      if (!helpOpen && !plan) review.undo();
      e.preventDefault();
      return;
    }
    if (e.metaKey || e.ctrlKey || e.altKey) return;
    if (e.key === '?') {
      helpOpen = !helpOpen;
      return;
    }
    if (helpOpen || plan) {
      if (e.key === 'Escape') {
        helpOpen = false;
        plan = null;
      }
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

  const filterLabels: [Filter, string][] = [
    ['all', 'All'],
    ['undecided', 'To review'],
    ['changed', 'Edited'],
  ];
  const filterCount = (f: Filter) => (f === 'all' ? review.counts.all : f === 'undecided' ? review.counts.undecided : review.counts.changed);
  const pct = (reviewed: number, all: number) => (all ? Math.round((reviewed / all) * 100) : 0);
</script>

<svelte:window onkeydown={onKey} />

<div class="app" class:in-detail={!!current} class:tuning={tuningOpen}>
  <header class="toolbar">
    <div class="left">
      <button class="btn plain" onclick={onExit}><Icon name="chevron-left" /> Folders</button>
    </div>
    <h1 class="folder" title={info?.root_path}>{folderName || 'Review'}</h1>
    <div class="right">
      <button
        class="btn"
        class:on={tuningOpen}
        aria-expanded={tuningOpen}
        onclick={() => (tuningOpen = !tuningOpen)}
        title="Change how strictly photos are grouped, and watch the review change"
      >
        <Icon name="sliders" /> Fine-tune
      </button>
      <button class="btn" onclick={() => review.undo()} disabled={!review.canUndo} title="Undo ({undoKey})"><Icon name="undo" /> Undo</button>
      <button
        class="btn"
        class:primary={review.markedCount > 0}
        onclick={() => (applyOpen = true)}
        title="Move the files you marked Quarantine into a quarantine folder (nothing is deleted)"
      >
        Apply{review.markedCount ? ` (${review.markedCount})` : ''}…
      </button>
      <button class="btn icon" onclick={() => (helpOpen = true)} title="Keyboard shortcuts (?)" aria-label="Keyboard shortcuts"><Icon name="help" /></button>
    </div>
  </header>

  {#if changes.length && info?.used}
    <div class="notice accent banner" role="status">
      <Icon name="info" size={18} />
      <p>
        <strong>Matching settings changed since this scan</strong> ({changes.join('; ')}), so what you see was found with the old values.
        Scanning again applies them; your keep/quarantine choices on files that haven't changed are kept.
        <button class="link" onclick={() => onRescan(info!.root_path, info!.used!.include_videos)}>Scan again</button>
      </p>
    </div>
  {/if}
  {#if moved}
    <div class="notice accent banner" role="status">
      <Icon name="info" size={18} />
      <p>Files were moved to quarantine, so what you see no longer matches the folder. Go back and scan it again to refresh.</p>
    </div>
  {/if}
  {#if review.error}
    <div class="notice danger banner" role="alert">
      <Icon name="info" size={18} />
      <p>{review.error} <button class="link" onclick={() => (review.error = '')}>Dismiss</button></p>
    </div>
  {/if}

  <div class="body">
    <nav class="sidebar" aria-label="Categories">
      <ul>
        {#each tabs as t (t)}
          {@const c = review.countsByTab[t]}
          <li>
            <button class="item" aria-current={review.tab === t ? 'page' : undefined} onclick={() => setTab(t)} title={tabInfo[t].blurb}>
              <Icon name={icons[t]} size={18} />
              <span class="label">{tabInfo[t].title}</span>
              <span class="count" aria-label="{c.all} groups">{review.loaded ? c.all : ''}</span>
              {#if c.all > 0}
                <span class="mini" aria-hidden="true"><span style="width:{pct(c.reviewed, c.all)}%"></span></span>
              {/if}
            </button>
          </li>
        {/each}
      </ul>
      <div class="overall" aria-label="Overall progress">
        <div class="row"><span class="caption">Overall</span><span class="caption">{overall.reviewed} of {overall.all} reviewed</span></div>
        <progress max={overall.all || 1} value={overall.reviewed} aria-label="Overall progress"></progress>
      </div>
    </nav>

    <main class="content">
      {#if !review.loaded}
        <div class="head"><div class="skeleton" style="height:28px;width:240px"></div></div>
        <div class="skeleton-grid" aria-busy="true" aria-label="Loading">
          {#each Array(8) as _, i (i)}<div class="skeleton" style="aspect-ratio:4/3.4"></div>{/each}
        </div>
      {:else if current}
        <Detail
          {review}
          paused={helpOpen || !!plan}
          group={current}
          position={Math.max(ids.indexOf(current.id), 0) + 1}
          total={ids.length}
          onBack={back}
          onPrev={() => step(-1)}
          onNext={() => step(1)}
          onAccept={acceptAndNext}
          {upcoming}
        />
      {:else}
        <div class="head">
          <h2 class="title">{tabInfo[review.tab].title}</h2>
          <p class="secondary blurb">{tabInfo[review.tab].blurb}</p>
          {#if usedLine(review.tab, info?.used ?? null)}<p class="hint">{usedLine(review.tab, info?.used ?? null)}</p>{/if}
          <p class="caption narrow-only">{overall.reviewed} of {overall.all} reviewed overall</p>
        </div>

        <div class="controls">
          <div class="seg" role="group" aria-label="Filter">
            {#each filterLabels as [key, label] (key)}
              <button aria-pressed={review.filter === key} onclick={() => setFilter(key)}>{label}<span class="n">{filterCount(key)}</span></button>
            {/each}
          </div>
          <span class="spacer"></span>
          <label class="sort">
            <span class="hint">Sort</span>
            <select class="field" value={review.sort} onchange={(e) => review.setSort(e.currentTarget.value as Sort)}>
              <option value="oldest">Oldest first</option>
              <option value="newest">Newest first</option>
            </select>
          </label>
          <button class="btn" disabled={review.counts.undecided === 0} onclick={askAcceptAll}><Icon name="check" /> Accept all suggestions</button>
        </div>

        {#if review.visible.length === 0}
          <div class="empty">
            <Icon name={review.counts.all === 0 ? 'tray' : 'check'} size={36} />
            <p class="lead">{review.counts.all === 0 ? 'Nothing in this category' : 'All done here'}</p>
            <p class="hint">{review.counts.all === 0 ? 'This scan found nothing to show under this heading.' : 'Everything in this view has been reviewed.'}</p>
          </div>
        {:else}
          <Overview {review} {focusId} onOpen={open} onFocus={(id) => (focusId = id)} />
        {/if}
      {/if}
    </main>
    {#if tuningOpen}
      <Tuning {scanId} onChanged={tuned} onClose={() => (tuningOpen = false)} onRescan={() => onRescan(info?.root_path ?? '', info?.used?.include_videos ?? true)} />
    {/if}
  </div>
</div>

{#if plan}
  <!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
  <div class="scrim" onclick={() => (plan = null)}>
    <div class="sheet" role="dialog" aria-modal="true" aria-labelledby="accept-title" tabindex="-1" onclick={(e) => e.stopPropagation()}>
      <h2 id="accept-title" class="lead">Accept {plan.groups} suggestion{plan.groups === 1 ? '' : 's'}?</h2>
      <p class="secondary sheet-text">
        {#if plan.quarantine > 0}
          <strong>{plan.quarantine} file{plan.quarantine === 1 ? '' : 's'}</strong> would be marked for quarantine. Nothing moves until you choose
          Apply.
        {:else}
          Everything in these groups would be kept. Nothing moves until you choose Apply.
        {/if}
      </p>
      <div class="sheet-actions">
        <button class="btn" onclick={() => (plan = null)}>Cancel</button>
        <button class="btn primary" onclick={doAcceptAll}>Accept</button>
      </div>
    </div>
  </div>
{/if}

{#if applyOpen}
  <Apply {scanId} onClose={() => (applyOpen = false)} onChanged={() => (moved = true)} />
{/if}

{#if helpOpen}
  <!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
  <div class="scrim" onclick={() => (helpOpen = false)}>
    <div class="sheet" role="dialog" aria-modal="true" aria-labelledby="help-title" tabindex="-1" onclick={(e) => e.stopPropagation()}>
      <h2 id="help-title" class="lead">Keyboard shortcuts</h2>
      <dl class="keys">
        <dt><kbd>{undoKey}</kbd></dt><dd>Undo the last change</dd>
        <dt><kbd>←</kbd> <kbd>→</kbd></dt><dd>Previous / next group</dd>
        <dt><kbd>1</kbd>–<kbd>9</kbd></dt><dd>Toggle keep for that photo</dd>
        <dt><kbd>Enter</kbd></dt><dd>Accept the suggestion and go to the next</dd>
        <dt><kbd>Space</kbd></dt><dd>Large preview of the highlighted photo</dd>
        <dt><kbd>Esc</kbd></dt><dd>Close a preview, go back to the overview</dd>
        <dt><kbd>?</kbd></dt><dd>This list</dd>
      </dl>
      <div class="sheet-actions"><button class="btn primary" onclick={() => (helpOpen = false)}>Done</button></div>
    </div>
  </div>
{/if}

<style>
  .app {
    display: flex;
    flex-direction: column;
    height: 100vh;
    min-height: 0;
  }

  /* toolbar: back on the left, what you are looking at in the middle, actions on the right */
  .toolbar {
    display: grid;
    grid-template-columns: 1fr auto 1fr;
    align-items: center;
    gap: var(--s3);
    min-height: 52px;
    padding: var(--s2) var(--s4);
    background: color-mix(in srgb, var(--bg) 82%, transparent);
    backdrop-filter: saturate(1.4) blur(16px);
    border-bottom: 1px solid var(--separator);
    flex: none;
  }
  .toolbar .left,
  .toolbar .right {
    display: flex;
    align-items: center;
    gap: var(--s2);
    min-width: 0;
  }
  .toolbar .right {
    justify-content: flex-end;
  }
  .folder {
    margin: 0;
    font-size: var(--fs-lead);
    font-weight: 600;
    max-width: 38vw;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .banner {
    margin: var(--s3) var(--s4) 0;
    flex: none;
  }

  .body {
    flex: 1;
    min-height: 0;
    display: grid;
    grid-template-columns: 248px 1fr;
  }
  .in-detail .body {
    grid-template-columns: 1fr;
  }
  .btn.on {
    background: color-mix(in srgb, var(--accent) 16%, var(--surface));
    box-shadow: inset 0 0 0 1.5px var(--accent);
  }
  .tuning .body {
    grid-template-columns: 248px 1fr 320px;
  }
  .tuning.in-detail .body {
    grid-template-columns: 1fr 320px;
  }
  .in-detail .sidebar {
    display: none;
  }

  /* sidebar: the five categories, each with its own progress */
  .sidebar {
    display: flex;
    flex-direction: column;
    justify-content: space-between;
    gap: var(--s4);
    padding: var(--s4) var(--s3);
    background: var(--sidebar);
    border-right: 1px solid var(--separator);
    overflow-y: auto;
  }
  .sidebar ul {
    list-style: none;
    margin: 0;
    padding: 0;
    display: grid;
    gap: 2px;
  }
  .item {
    position: relative;
    display: grid;
    grid-template-columns: 18px 1fr auto;
    align-items: center;
    column-gap: var(--s3);
    width: 100%;
    padding: 7px var(--s3) 9px;
    border: 0;
    border-radius: 8px;
    background: transparent;
    color: var(--label);
    font: 500 var(--fs-body) / 1.2 var(--font);
    text-align: left;
    cursor: pointer;
    transition: background var(--t-fast) var(--ease);
  }
  .item:hover {
    background: color-mix(in srgb, var(--label) 7%, transparent);
  }
  .item[aria-current='page'] {
    background: var(--accent);
    color: var(--accent-fg);
  }
  .item .label {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .item .count {
    font-variant-numeric: tabular-nums;
    opacity: 0.75;
    font-size: var(--fs-small);
  }
  .mini {
    grid-column: 2 / -1;
    height: 3px;
    margin-top: 6px;
    border-radius: 2px;
    background: color-mix(in srgb, currentColor 18%, transparent);
    overflow: hidden;
  }
  .mini span {
    display: block;
    height: 100%;
    background: currentColor;
    opacity: 0.85;
    transition: width var(--t) var(--ease);
  }
  .overall {
    display: grid;
    gap: 6px;
    padding: var(--s3);
    border-radius: var(--radius);
    background: var(--surface);
    box-shadow: var(--shadow-sm);
  }
  .overall .row {
    display: flex;
    justify-content: space-between;
    gap: var(--s2);
  }

  .content {
    min-width: 0;
    overflow-y: auto;
    padding: var(--s5) var(--s6) var(--s7);
  }
  .head {
    display: grid;
    gap: var(--s1);
    margin-bottom: var(--s4);
  }
  .blurb {
    max-width: 70ch;
  }
  .narrow-only {
    display: none;
  }
  .controls {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: var(--s3);
    margin-bottom: var(--s5);
  }
  .spacer {
    flex: 1;
  }
  .sort {
    display: inline-flex;
    align-items: center;
    gap: var(--s2);
  }
  .skeleton-grid {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(220px, 1fr));
    gap: var(--s4);
  }
  .empty {
    display: grid;
    justify-items: center;
    gap: var(--s2);
    padding: var(--s7) 0;
    color: var(--label-3);
    text-align: center;
  }

  .sheet-text {
    margin: var(--s2) 0 var(--s4);
  }
  .sheet-actions {
    display: flex;
    justify-content: flex-end;
    gap: var(--s2);
    margin-top: var(--s4);
  }
  .keys {
    display: grid;
    grid-template-columns: auto 1fr;
    gap: var(--s3) var(--s5);
    margin: var(--s4) 0 0;
    align-items: center;
  }
  .keys dt {
    display: flex;
    gap: 4px;
    align-items: center;
    justify-content: flex-end;
  }
  .keys dd {
    margin: 0;
  }

  /* narrow windows: the sidebar becomes a scrolling strip above the content */
  @media (max-width: 860px) {
    .body,
    .tuning .body,
    .tuning.in-detail .body {
      grid-template-columns: 1fr;
      grid-template-rows: auto 1fr;
    }
    .tuning.in-detail .body {
      grid-template-rows: 1fr;
    }
    .sidebar {
      flex-direction: row;
      align-items: stretch;
      padding: var(--s2) var(--s3);
      border-right: 0;
      border-bottom: 1px solid var(--separator);
      overflow-x: auto;
      overflow-y: hidden;
    }
    .sidebar ul {
      grid-auto-flow: column;
      grid-auto-columns: max-content;
      gap: var(--s1);
    }
    .overall {
      display: none;
    }
    .narrow-only {
      display: block;
    }
    .content {
      padding: var(--s4);
    }
    .toolbar {
      grid-template-columns: auto 1fr auto;
    }
    .folder {
      text-align: center;
      max-width: none;
    }
  }
  @media (max-width: 640px) {
    .toolbar :global(.btn:not(.icon):not(.primary)) {
      padding: 0 var(--s2);
    }
    .controls .spacer {
      display: none;
    }
  }
</style>
