<script lang="ts">
  import { onMount } from 'svelte';
  import { api, ApiError, type Batch, type BatchDetail, type BatchItem, type FileDto, type UndoResult } from './lib/api';
  import { formatBytes } from './lib/review.svelte';
  import Lightbox from './Lightbox.svelte';
  import Icon from './Icon.svelte';

  let { onClose, onChanged }: { onClose: () => void; onChanged: (reviewUpdated: boolean) => void } = $props();

  let batches = $state<Batch[]>([]);
  let loaded = $state(false);
  let openId = $state<string | null>(null);
  let detail = $state<BatchDetail | null>(null);
  let picked = $state<Set<number>>(new Set());
  let busy = $state(false);
  let note = $state<UndoResult | null>(null);
  let error = $state('');
  let viewing = $state<number | null>(null);

  type Kind = 'all' | 'photo' | 'video';
  type Order = 'moved' | 'oldest' | 'newest' | 'name' | 'largest' | 'smallest';
  let kind = $state<Kind>('all');
  let order = $state<Order>('moved');

  const say = (e: unknown) => (e instanceof ApiError ? e.message : e instanceof Error ? e.message : String(e));
  const when = (iso: string) => new Date(iso).toLocaleString(undefined, { dateStyle: 'medium', timeStyle: 'short' });
  const name = (p: string) => p.slice(p.lastIndexOf('/') + 1);
  const stateText: Record<Batch['state'], string> = {
    running: 'Interrupted',
    done: 'In quarantine',
    partially_undone: 'Partly restored',
    undone: 'All restored',
  };

  // Newest first, and the batches that still hold something come before the emptied ones.
  const ordered = $derived([...batches].sort((a, b) => Number(b.in_quarantine > 0) - Number(a.in_quarantine > 0) || b.created_at.localeCompare(a.created_at)));
  const isVideo = (i: BatchItem) => i.media_class === 'Video';
  // The date a photo was taken, or failing that the file's own date.
  const dateOf = (i: BatchItem) => i.taken_at ?? new Date(i.mtime_unix * 1000).toISOString().replace('T', ' ').slice(0, 19);
  const day = (i: BatchItem) => dateOf(i).slice(0, 10);
  // What the filter and the order leave of this batch: the grid shows it, and "select all" and
  // "put back all" mean these files.
  const shown = $derived.by(() => {
    const out = (detail?.items ?? []).filter((i) => (kind === 'photo' ? !isVideo(i) : kind === 'video' ? isVideo(i) : true));
    if (order === 'oldest' || order === 'newest') {
      out.sort((a, b) => (dateOf(a) < dateOf(b) ? -1 : dateOf(a) > dateOf(b) ? 1 : 0));
      if (order === 'newest') out.reverse();
    } else if (order === 'name') out.sort((a, b) => name(a.relative_path).localeCompare(name(b.relative_path), undefined, { numeric: true }));
    else if (order === 'largest') out.sort((a, b) => b.size_bytes - a.size_bytes);
    else if (order === 'smallest') out.sort((a, b) => a.size_bytes - b.size_bytes);
    return out;
  });
  const held = $derived(shown.filter((i) => i.state === 'moved'));
  const heldBytes = $derived(held.reduce((n, i) => n + i.size_bytes, 0));
  const videosHere = $derived((detail?.items ?? []).filter((i) => i.state === 'moved' && isVideo(i)).length);
  const photosHere = $derived((detail?.items ?? []).filter((i) => i.state === 'moved' && !isVideo(i)).length);
  const totalHeld = $derived(batches.reduce((n, b) => n + b.in_quarantine, 0));

  // a selection never includes files the filter has hidden
  $effect(() => {
    kind;
    picked = new Set();
  });

  async function loadBatches(keepOpen = true) {
    try {
      batches = await api.batches();
      if (!keepOpen || openId === null || !batches.some((b) => b.id === openId)) {
        openId = ordered[0]?.id ?? null;
      }
    } catch (e) {
      error = say(e);
    } finally {
      loaded = true;
    }
  }

  async function loadDetail() {
    detail = null;
    picked = new Set();
    kind = 'all';
    if (openId === null) return;
    try {
      detail = await api.batch(openId);
    } catch (e) {
      error = say(e);
    }
  }

  onMount(async () => {
    await loadBatches(false);
    await loadDetail();
  });

  async function choose(id: string) {
    openId = id;
    note = null;
    await loadDetail();
  }

  function toggle(seq: number) {
    const next = new Set(picked);
    if (next.has(seq)) next.delete(seq);
    else next.add(seq);
    picked = next;
  }
  const allPicked = $derived(held.length > 0 && held.every((i) => picked.has(i.seq)));
  const pickAll = () => (picked = allPicked ? new Set() : new Set(held.map((i) => i.seq)));

  async function restore(seqs: number[]) {
    if (!openId || seqs.length === 0) return;
    busy = true;
    error = '';
    note = null;
    try {
      note = await api.restoreItems(openId, seqs);
      onChanged(note.review_updated);
      await loadBatches();
      await loadDetail();
    } catch (e) {
      error = say(e);
    } finally {
      busy = false;
    }
  }

  const asFile = (i: BatchItem): FileDto => ({
    id: String(i.seq),
    relative_path: i.relative_path,
    file_name: name(i.relative_path),
    media_class: i.media_class,
    size_bytes: i.size_bytes,
    is_placeholder: false,
  });
  const viewable = $derived(held.filter((i) => i.media_class === 'Image' || i.media_class === 'Video' || i.media_class === 'Raw'));
  const viewFiles = $derived(viewable.map(asFile));
  const viewIndex = $derived(viewing === null ? 0 : Math.max(0, viewable.findIndex((i) => i.seq === viewing)));

  // The thumbnail of a file that is not an image or video (a companion file) is an icon.
  const hasPicture = (i: BatchItem) => i.media_class !== 'Sidecar';

  // Capture phase: while this panel is open it owns the keyboard.
  function onKey(e: KeyboardEvent) {
    if (viewing !== null) return;
    if (e.key === 'Escape') onClose();
    if (!(e.target as HTMLElement).closest('button, input, select')) e.stopImmediatePropagation();
  }

  // Thumbnails load as they come into view, so a batch of thousands opens at once.
  const lazy = (node: HTMLImageElement) => {
    node.loading = 'lazy';
  };
</script>

<svelte:window onkeydowncapture={onKey} />

<!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
<div class="scrim" onclick={onClose}>
  <div class="sheet wide q" role="dialog" aria-modal="true" aria-labelledby="q-title" tabindex="-1" onclick={(e) => e.stopPropagation()}>
    <header>
      <h2 id="q-title" class="title">Quarantine</h2>
      <button class="btn icon plain" onclick={onClose} aria-label="Close"><Icon name="x" /></button>
    </header>

    {#if error}<div class="notice danger" role="alert"><Icon name="info" size={18} /><p>{error}</p></div>{/if}

    {#if !loaded}
      <div class="skeleton" style="height:160px"></div>
    {:else if batches.length === 0}
      <div class="empty">
        <Icon name="archive" size={36} />
        <p class="lead">Nothing is in quarantine</p>
        <p class="hint">Files you apply as Quarantine are moved into a folder inside the scanned folder and listed here. Nothing is ever deleted.</p>
      </div>
    {:else}
      <p class="secondary lead-in">
        {totalHeld} file{totalHeld === 1 ? '' : 's'} set aside. Put back any of them, one by one or all at once; nothing is deleted and nothing is overwritten.
      </p>
      <div class="cols">
        <ul class="batches" aria-label="Quarantine batches">
          {#each ordered as b (b.id)}
            <li>
              <button class="batch" aria-current={openId === b.id ? 'true' : undefined} onclick={() => choose(b.id)}>
                <strong>{when(b.created_at)}</strong>
                <span class="hint">{b.in_quarantine} in quarantine{b.in_quarantine ? ` · ${formatBytes(b.in_quarantine_bytes)}` : ''}</span>
                <span class="hint">{stateText[b.state]}{b.restored ? ` · ${b.restored} restored` : ''}</span>
              </button>
            </li>
          {/each}
        </ul>

        <section class="files" aria-label="Files in this batch">
          {#if !detail}
            <div class="skeleton" style="height:200px"></div>
          {:else}
            <div class="bar rowbar">
              <div class="l">
              <label class="all">
                <input type="checkbox" checked={allPicked} disabled={held.length === 0} onchange={pickAll} />
                <span>{picked.size ? `${picked.size} selected` : 'Select all'}</span>
              </label>
              <span class="hint">{formatBytes(heldBytes)}</span>
              <div class="seg" role="group" aria-label="Kind">
                <button aria-pressed={kind === 'all'} onclick={() => (kind = 'all')}>All<span class="n">{photosHere + videosHere}</span></button>
                <button aria-pressed={kind === 'photo'} onclick={() => (kind = 'photo')}>Photos<span class="n">{photosHere}</span></button>
                <button aria-pressed={kind === 'video'} onclick={() => (kind = 'video')}>Videos<span class="n">{videosHere}</span></button>
              </div>
              <label class="sort">
                <span class="hint">Sort</span>
                <select class="field" bind:value={order}>
                  <option value="moved">As moved</option>
                  <option value="oldest">Date, oldest first</option>
                  <option value="newest">Date, newest first</option>
                  <option value="name">Name</option>
                  <option value="largest">Largest first</option>
                  <option value="smallest">Smallest first</option>
                </select>
              </label>
              </div>
              <div class="r">
              <button class="btn" disabled={busy || picked.size === 0} onclick={() => restore([...picked])}>
                <Icon name="undo" /> Put back selected
              </button>
              <button class="btn" disabled={busy || held.length === 0} onclick={() => restore(held.map((i) => i.seq))}>
                <Icon name="undo" /> {kind === 'all' ? 'Put back all' : `Put back all ${kind === 'photo' ? 'photos' : 'videos'}`}
              </button>
              </div>
            </div>
            {#if note}
              <p class="notice success" role="status">
                <Icon name="check" size={16} />
                <span>
                  Put back {note.restored} file{note.restored === 1 ? '' : 's'}.
                  {#if note.skipped.length}{note.skipped.length} could not be put back and stay here: {note.skipped[0].reason}.{/if}
                  {#if note.restored > 0 && note.review_updated} The review shows them again.{/if}
                </span>
              </p>
            {/if}

            <ul class="grid">
              {#each shown as i (i.seq)}
                {@const isHeld = i.state === 'moved'}
                <li class:gone={!isHeld}>
                  <div class="card">
                    <button
                      class="pic"
                      disabled={!isHeld || !hasPicture(i)}
                      onclick={() => (viewing = i.seq)}
                      title={isHeld ? 'Look at it' : 'Already put back'}
                    >
                      {#if hasPicture(i) && isHeld}
                        <img src={api.quarantineUrl(detail.id, i.seq, 'thumb')} alt="" decoding="async" use:lazy />
                      {:else}
                        <Icon name={isHeld ? 'archive' : 'check'} size={28} />
                      {/if}
                    </button>
                    {#if isHeld}
                      <input class="pick" type="checkbox" checked={picked.has(i.seq)} onchange={() => toggle(i.seq)} aria-label="Select {name(i.relative_path)}" />
                    {/if}
                    <div class="meta">
                      <span class="nm" title={i.relative_path}>{name(i.relative_path)}</span>
                      <span class="hint">
                        {day(i)} · {formatBytes(i.size_bytes)}{i.is_sibling ? ' · companion' : ''}{!isHeld ? (i.state === 'restored' ? ' · put back' : ' · not moved') : ''}
                      </span>
                    </div>
                  </div>
                </li>
              {/each}
            </ul>
          {/if}
        </section>
      </div>
    {/if}
  </div>
</div>

{#if viewing !== null && detail}
  {@const batchId = detail.id}
  <Lightbox
    files={viewFiles}
    index={viewIndex}
    onIndex={(i) => (viewing = viewable[i].seq)}
    onClose={() => (viewing = null)}
    context="In quarantine"
    urls={(f) => ({ low: api.quarantineUrl(batchId, Number(f.id), 'preview', 1600), high: api.quarantineUrl(batchId, Number(f.id), 'preview', 16384) })}
  />
{/if}

<style>
  .sheet.wide.q {
    width: min(calc(1120px * var(--k)), 100%);
    display: grid;
    gap: var(--s3);
    max-height: calc(100vh - 2 * var(--s5));
    grid-template-rows: auto auto minmax(0, 1fr);
  }
  header {
    display: flex;
    align-items: center;
    justify-content: space-between;
  }
  .lead-in {
    margin: 0;
  }
  .cols {
    display: grid;
    grid-template-columns: calc(230px * var(--k)) minmax(0, 1fr);
    gap: var(--s4);
    min-height: 0;
  }
  .batches {
    list-style: none;
    margin: 0;
    padding: 0;
    display: grid;
    gap: var(--s2);
    align-content: start;
    overflow-y: auto;
  }
  .batch {
    display: grid;
    gap: 2px;
    width: 100%;
    padding: var(--s3);
    border: 0;
    border-radius: var(--radius);
    background: var(--surface-2);
    color: inherit;
    text-align: left;
    cursor: pointer;
  }
  .batch[aria-current='true'] {
    box-shadow: inset 0 0 0 2px var(--accent);
  }
  .files {
    display: grid;
    gap: var(--s3);
    grid-template-rows: auto auto minmax(0, 1fr);
    min-height: 0;
  }
  /* once the row cannot hold both groups on a line, the right group's controls sit one over another,
     aligned right (em = the row's font size, so this follows the UI scale) */
  @container (max-width: 66em) {
    .bar .r {
      flex-direction: column;
      align-items: flex-end;
    }
  }
  .all {
    display: inline-flex;
    align-items: center;
    gap: var(--s2);
    font-weight: 600;
  }
  .sort {
    display: inline-flex;
    align-items: center;
    gap: var(--s2);
  }
  .grid {
    list-style: none;
    margin: 0;
    padding: 0;
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(calc(150px * var(--k)), 1fr));
    gap: var(--s3);
    overflow-y: auto;
    align-content: start;
    min-height: 0;
  }
  .card {
    position: relative;
    display: grid;
    gap: var(--s2);
  }
  .gone {
    opacity: 0.5;
  }
  .pic {
    display: grid;
    place-items: center;
    aspect-ratio: 1;
    padding: 0;
    border: 0;
    border-radius: var(--radius);
    overflow: hidden;
    background: #000;
    color: var(--label-3);
    cursor: zoom-in;
  }
  .pic:disabled {
    cursor: default;
    background: var(--surface-2);
  }
  .pic img {
    width: 100%;
    height: 100%;
    object-fit: contain;
  }
  .pick {
    position: absolute;
    top: var(--s2);
    left: var(--s2);
    width: calc(18px * var(--k));
    height: calc(18px * var(--k));
  }
  .meta {
    display: grid;
    min-width: 0;
  }
  .nm {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    font-size: var(--fs-small);
    font-weight: 600;
  }
  .empty {
    display: grid;
    justify-items: center;
    gap: var(--s2);
    padding: var(--s6) 0;
    text-align: center;
  }
  .notice {
    display: flex;
    align-items: center;
    gap: var(--s2);
    margin: 0;
  }
  @media (max-width: 760px) {
    .cols {
      grid-template-columns: 1fr;
    }
  }
</style>
