<script lang="ts">
  import { api, type FileDto } from './lib/api';
  import { formatBytes, formatWhen, reasonText, type Group, type Review } from './lib/review.svelte';
  import Lightbox from './Lightbox.svelte';
  import Icon from './Icon.svelte';

  let {
    review,
    paused,
    group,
    position,
    total,
    onBack,
    onPrev,
    onNext,
    onAccept,
    upcoming = [],
  }: {
    review: Review;
    /** A dialog is open elsewhere: ignore keys. */
    paused: boolean;
    group: Group;
    position: number;
    total: number;
    onBack: () => void;
    onPrev: () => void;
    onNext: () => void;
    onAccept: () => void;
    /** Photos of the next groups: fetched quietly now so they are ready when you get there. */
    upcoming?: FileDto[];
  } = $props();

  let active = $state(0); // highlighted member: target of Space
  let previewing = $state<number | null>(null);

  // A new group starts with the first member highlighted and no preview.
  $effect(() => {
    group.id;
    active = 0;
    previewing = null;
  });

  const status = $derived(review.statuses.get(group.id) ?? 'undecided');
  const isBurst = $derived(group.tab === 'burst');
  const sharpest = $derived(Math.max(0, ...Object.values(group.sharpness)));
  const undoKey = /Mac|iPhone|iPad/i.test(navigator.userAgent) ? '⌘Z' : 'Ctrl+Z';
  const undecided = $derived(status === 'undecided' || status === 'partial');
  const keepsAll = $derived(group.tab === 'similar' || group.tab === 'orphans');
  const primaryLabel = $derived(!undecided ? 'Next' : isBurst ? 'Keep suggested only' : keepsAll ? 'Keep all' : 'Accept suggestion');

  // While the preview is open it handles its own keys (see Lightbox).
  function onKey(e: KeyboardEvent) {
    if (paused || previewing !== null || e.metaKey || e.ctrlKey || e.altKey) return;
    if (/^[1-9]$/.test(e.key)) {
      const m = group.members[Number(e.key) - 1];
      if (m) {
        active = Number(e.key) - 1;
        review.toggle(group, m);
        e.preventDefault();
      }
    } else if (e.key === ' ' && (e.target as HTMLElement).tagName !== 'BUTTON') {
      previewing = active;
      e.preventDefault();
    }
  }

  // The server makes a sharp preview the first time it is asked for one (a few
  // seconds for a big HEIC), so ask for the next groups' previews while the
  // current one is being looked at. Skipped on a slow connection or when the
  // browser has asked to save data.
  const warmed = new Set<string>();
  $effect(() => {
    const save = (navigator as unknown as { connection?: { saveData?: boolean } }).connection?.saveData;
    if (save) return;
    const timer = setTimeout(() => {
      for (const f of upcoming) {
        if (f.media_class === 'Video' || warmed.has(f.id)) continue;
        warmed.add(f.id);
        new Image().src = api.fileUrl(f.id, 'preview');
      }
    }, 600); // let the photos in front of you load first
    return () => clearTimeout(timer);
  });

  /** The segmented control sets a side; clicking the side already chosen does nothing. */
  function choose(i: number, wantKeep: boolean, e: MouseEvent) {
    // A mouse click gives up the focus it took, so Enter and the number keys
    // keep working as shortcuts afterwards; a keyboard "click" (detail 0)
    // keeps focus where the person is.
    if (e.detail > 0) (e.currentTarget as HTMLElement).blur();
    active = i;
    const m = group.members[i];
    if (review.effective(group, m) !== wantKeep) review.toggle(group, m);
  }

  const dims = (a: { width: number | null; height: number | null } | null) => (a?.width && a?.height ? `${a.width} × ${a.height}` : '');
  const minutes = (s: number) => `${Math.floor(s / 60)}:${String(Math.round(s % 60)).padStart(2, '0')}`;
  // Which photos' sharp previews have arrived (until then a small spinner says why it looks soft).
  let sharp = $state<Record<string, boolean>>({});
  const loaded = (id: string) => (e: Event) => {
    (e.currentTarget as HTMLImageElement).classList.add('in');
    sharp[id] = true;
  };
  const ready = (node: HTMLImageElement, id: string) => {
    if (node.complete && node.naturalWidth) {
      node.classList.add('in');
      sharp[id] = true;
    }
  };
</script>

<svelte:window onkeydown={onKey} />

<section class="detail">
  <div class="bar">
    <div class="nav">
      <button class="btn" onclick={onBack}><Icon name="chevron-left" /> Overview</button>
      <div class="seg pager" role="group" aria-label="Navigate groups">
        <button onclick={onPrev} disabled={position <= 1} title="Previous (←)" aria-label="Previous group"><Icon name="chevron-left" /></button>
        <span class="pos" aria-live="polite">{position} of {total}</span>
        <button onclick={onNext} disabled={position >= total} title="Next (→)" aria-label="Next group"><Icon name="chevron-right" /></button>
      </div>
    </div>

    <div class="summary">
      <strong>{formatWhen(group.capturedAt)}</strong>
      <span class="hint">{group.label}{group.reasons.length ? ` · ${[...new Set(group.reasons.map(reasonText))].join(', ')}` : ''}</span>
    </div>

    <div class="actions">
      {#if isBurst}
        <button class="btn" onclick={() => review.keepAll(group)} title="Keep every frame in this sequence">Keep all</button>
      {/if}
      <button
        class="btn"
        disabled={review.resetPaths([group]).length === 0}
        onclick={() => review.reset([group])}
        title="Forget your choices for this group and go back to the suggestion ({undoKey} undoes it)"
      >
        Reset
      </button>
      <button class="btn primary" onclick={onAccept} title="{primaryLabel} and continue (Enter)">{primaryLabel} <kbd class="on-accent">⏎</kbd></button>
    </div>
  </div>

  <div class="members" style="--n:{group.members.length}">
    {#each group.members as m, i (m.id)}
      {@const keep = review.effective(group, m)}
      <article class:aside={!keep} class:active={active === i}>
        <button class="photo" onclick={() => { active = i; previewing = i; }}  title="Large preview (Space)">
          <span class="backdrop" style="background-image:url({api.fileUrl(m.id, 'thumb')})" aria-hidden="true"></span>
          <!-- the small thumbnail is already cached (the overview used it), so the view opens at once;
               the sharp preview fades in over it when it arrives -->
          <img class="low" src={api.fileUrl(m.id, 'thumb')} alt="" decoding="async" aria-hidden="true" />
          <img class="high" src={api.fileUrl(m.id, 'preview')} alt={m.file_name} decoding="async" onload={loaded(m.id)} use:ready={m.id} />
          {#if !sharp[m.id] && m.media_class !== 'Video'}<span class="loading" role="status"><span class="spinner"></span> Loading full resolution…</span>{/if}
          {#if i < 9}<kbd class="idx">{i + 1}</kbd>{/if}
          <span class="flags">
            {#if group.suggested.has(m.id) && !isBurst}<span class="badge overlay accent"><Icon name="check" size={11} /> Suggested</span>{/if}
            {#if isBurst && group.suggested.has(m.id)}<span class="badge overlay accent"><Icon name="sparkles" size={11} /> Sharpest</span>{/if}
          </span>
          {#if !keep}<span class="dim" aria-hidden="true"></span><span class="veil"><span class="badge overlay warning">Will be quarantined</span></span>{/if}
        </button>

        <div class="meta">
          <div class="info">
            <strong class="name" title={m.relative_path}>{m.file_name}</strong>
            {#await review.analysis(m.id) then a}
              <span class="line">{[dims(a), a?.duration_seconds ? minutes(a.duration_seconds) : '', formatBytes(m.size_bytes)].filter(Boolean).join(' · ')}</span>
              {#if !isBurst}<span class="hint">{[a?.origin?.replaceAll('_', ' '), a?.camera_model].filter(Boolean).join(' · ')}</span>{/if}
            {/await}
            {#if isBurst && group.sharpness[m.id] !== undefined}
              <span class="sharp" title="Sharpness (higher is sharper)">
                <span class="bar-fill" style:width="{sharpest ? (group.sharpness[m.id] / sharpest) * 100 : 0}%"></span>
              </span>
              <span class="hint">Sharpness {Math.round(group.sharpness[m.id])}</span>
            {/if}
          </div>
          <div class="seg decide" role="group" aria-label="Decision for {m.file_name}">
            <button class="keep" aria-pressed={keep} aria-label="Keep" title="Keep" onclick={(e) => choose(i, true, e)}><Icon name="check" size={16} /></button>
            <button class="aside" aria-pressed={!keep} aria-label="Quarantine" title="Quarantine (moved aside, never deleted)" onclick={(e) => choose(i, false, e)}><Icon name="archive" size={16} /></button>
          </div>
        </div>
      </article>
    {/each}
  </div>

  <p class="hint foot">
    <Icon name="info" size={13} /> Quarantined files are moved aside, never deleted, and only when you apply your decisions. Anything marked Keep stays
    where it is.
  </p>
</section>

{#if previewing !== null}
  <Lightbox files={group.members} index={previewing} onIndex={(i) => ((previewing = i), (active = i))} onClose={() => (previewing = null)} />
{/if}

<style>
  .detail {
    display: grid;
    gap: var(--s4);
  }
  .bar {
    display: grid;
    grid-template-columns: auto 1fr auto;
    align-items: center;
    gap: var(--s4);
  }
  .nav,
  .actions {
    display: flex;
    align-items: center;
    gap: var(--s2);
  }
  .pager {
    align-items: center;
  }
  .pager button {
    display: grid;
    place-items: center;
    padding: 0 var(--s2);
  }
  .pager button:disabled {
    opacity: 0.35;
    cursor: default;
  }
  .pos {
    padding: 0 var(--s2);
    font-variant-numeric: tabular-nums;
    font-weight: 600;
    font-size: var(--fs-small);
  }
  .summary {
    display: grid;
    gap: 1px;
    justify-items: center;
    text-align: center;
    min-width: 0;
  }
  .summary .hint {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    max-width: 100%;
  }
  .on-accent {
    background: rgb(255 255 255 / 0.22);
    color: inherit;
    box-shadow: none;
  }

  .members {
    display: grid;
    grid-template-columns: repeat(auto-fit, minmax(min(100%, 280px), 1fr));
    gap: var(--s4);
    align-items: start;
  }
  article {
    display: grid;
    gap: var(--s3);
    padding: var(--s3);
    border-radius: var(--radius-lg);
    background: var(--surface);
    box-shadow: var(--shadow-sm);
    transition: box-shadow var(--t) var(--ease);
  }
  article.active {
    box-shadow: 0 0 0 2px var(--accent), var(--shadow-sm);
  }
  .photo {
    position: relative;
    display: grid;
    place-items: center;
    height: clamp(240px, 46vh, 520px);
    padding: 0;
    border: 0;
    border-radius: var(--radius);
    overflow: hidden;
    background: var(--surface-2);
    cursor: zoom-in;
  }
  /* the photo's own colours, blurred, fill the space around it instead of grey bars */
  .backdrop {
    position: absolute;
    inset: -20px;
    background-size: cover;
    background-position: center;
    filter: blur(26px) saturate(1.2) brightness(0.88);
    opacity: 0.9;
  }
  .photo img {
    grid-area: 1 / 1;
    position: relative;
    width: 100%;
    height: 100%;
    object-fit: contain;
    filter: drop-shadow(0 4px 14px rgb(0 0 0 / 0.35));
  }
  /* the placeholder is always there; the sharp one fades in on top of it */
  .photo img.high {
    opacity: 0;
    transition: opacity 0.3s var(--ease);
  }
  .photo img.high:global(.in) {
    opacity: 1;
  }
  .loading {
    position: absolute;
    left: 50%;
    bottom: var(--s2);
    translate: -50% 0;
    display: inline-flex;
    align-items: center;
    gap: var(--s2);
    padding: 3px 10px 3px 8px;
    border-radius: 999px;
    background: rgb(20 20 22 / 0.72);
    color: #fff;
    font-size: var(--fs-caption);
    font-weight: 600;
    white-space: nowrap;
    backdrop-filter: blur(8px);
  }
  .loading .spinner {
    width: 11px;
    height: 11px;
    border-width: 1.5px;
    border-color: rgb(255 255 255 / 0.35);
    border-top-color: #fff;
  }
  .idx {
    position: absolute;
    top: var(--s2);
    left: var(--s2);
    background: rgb(20 20 22 / 0.72);
    color: #fff;
    box-shadow: none;
    backdrop-filter: blur(8px);
  }
  .flags {
    position: absolute;
    top: var(--s2);
    right: var(--s2);
    display: flex;
    gap: var(--s1);
  }
  /* A quarantined photo is dimmed by an overlay that leaves the DOM when you
     choose Keep. (A class-toggled CSS filter on the image stayed applied in
     WebKit, the desktop app's engine, after the class was removed.) */
  .dim {
    position: absolute;
    inset: 0;
    border-radius: inherit;
    background: rgb(70 70 78 / 0.5);
    pointer-events: none;
  }
  .veil {
    position: absolute;
    left: var(--s2);
    bottom: var(--s2);
  }
  .meta {
    display: flex;
    align-items: flex-start;
    justify-content: space-between;
    gap: var(--s3);
  }
  .info {
    display: grid;
    gap: 2px;
    min-width: 0;
    flex: 1;
  }
  /* the decision: a small pill of two icons. The chosen side is tinted in its
     meaning's colour; the other stays quiet. */
  .decide {
    flex: none;
    padding: 3px;
    gap: 3px;
    border-radius: 999px;
  }
  .decide > button {
    display: grid;
    place-items: center;
    width: 34px;
    min-height: 28px;
    padding: 0;
    border-radius: 999px;
    color: var(--label-3);
  }
  .decide > button:hover {
    color: var(--label);
  }
  .decide > button:focus-visible {
    box-shadow: var(--ring);
  }
  .decide > button.keep[aria-pressed='true'] {
    background: color-mix(in srgb, var(--success) 18%, var(--surface));
    color: var(--success);
    box-shadow: inset 0 0 0 1.5px var(--success);
  }
  .decide > button.aside[aria-pressed='true'] {
    background: color-mix(in srgb, var(--warning) 20%, var(--surface));
    color: var(--warning-text, var(--warning));
    box-shadow: inset 0 0 0 1.5px var(--warning);
  }
  .name {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .line {
    color: var(--label-2);
    font-variant-numeric: tabular-nums;
  }
  .sharp {
    display: block;
    height: 4px;
    margin-top: var(--s1);
    border-radius: 2px;
    background: var(--surface-2);
    overflow: hidden;
  }
  .bar-fill {
    display: block;
    height: 100%;
    min-width: 2px;
    background: var(--accent);
    border-radius: 2px;
  }
  .foot {
    display: flex;
    align-items: center;
    gap: var(--s2);
    max-width: 80ch;
  }

  @media (max-width: 760px) {
    .bar {
      grid-template-columns: 1fr;
      justify-items: stretch;
    }
    .nav,
    .actions {
      justify-content: space-between;
    }
    .summary {
      order: -1;
    }
    .photo {
      height: clamp(200px, 38vh, 360px);
    }
  }
</style>
