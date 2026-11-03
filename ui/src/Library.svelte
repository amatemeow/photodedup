<script lang="ts">
  import { api, type FileDto, type LibraryFile } from './lib/api';
  import { formatBytes, mediaOf, tabInfo, type Group, type Review } from './lib/review.svelte';
  import Lightbox from './Lightbox.svelte';
  import Detail from './Detail.svelte';
  import Icon from './Icon.svelte';
  import { ImageQueue } from './lib/imageQueue';

  let { review, onOpenGroup }: { review: Review; onOpenGroup: (g: Group) => void } = $props();

  let files = $state.raw<LibraryFile[]>([]);
  let loaded = $state(false);
  let error = $state('');

  type Scope = 'all' | 'grouped' | 'alone' | 'marked';
  type Kind = 'all' | 'photo' | 'video';
  let scope = $state<Scope>('all');
  let kind = $state<Kind>('all');
  let newestFirst = $state(false);

  // Fetched again whenever the review is (files moved to or back from quarantine, matching tuned).
  $effect(() => {
    review.version;
    api
      .library(review.scanId)
      .then((list) => {
        files = list;
        error = '';
      })
      .catch((e) => (error = e instanceof Error ? e.message : String(e)))
      .finally(() => (loaded = true));
  });

  const when = (f: LibraryFile) => f.captured_at ?? new Date(f.mtime_unix * 1000).toISOString().replace('T', ' ').slice(0, 19);
  const isVideo = (f: LibraryFile) => f.media_class === 'Video';
  const marked = (f: LibraryFile) => review.decisions[f.relative_path] === false;

  const shown = $derived.by(() => {
    // Reading `review.decisions` here makes the list follow a mark made anywhere.
    const out = files.filter((f) => {
      if (kind === 'photo' && isVideo(f)) return false;
      if (kind === 'video' && !isVideo(f)) return false;
      if (scope === 'grouped') return review.groupsFor(f.relative_path).length > 0;
      if (scope === 'alone') return review.groupsFor(f.relative_path).length === 0;
      if (scope === 'marked') return marked(f);
      return true;
    });
    out.sort((a, b) => (when(a) < when(b) ? -1 : when(a) > when(b) ? 1 : 0));
    return newestFirst ? out.reverse() : out;
  });

  const counts = $derived({
    all: files.length,
    grouped: files.filter((f) => review.groupsFor(f.relative_path).length > 0).length,
    marked: files.filter(marked).length,
  });

  // ---- a virtual grid: only the rows in (or near) view exist in the page ----
  let scroller: HTMLElement | undefined = $state();
  let width = $state(900);
  let height = $state(600);
  let top = $state(0);
  const gap = 8;
  const cell = $derived(Math.round(150 * parseFloat(getComputedStyle(document.documentElement).getPropertyValue('--k') || '1')));
  const cols = $derived(Math.max(1, Math.floor((width + gap) / (cell + gap))));
  const size = $derived((width - (cols - 1) * gap) / cols);
  const rowH = $derived(size + gap);
  const rows = $derived(Math.ceil(shown.length / cols));
  const first = $derived(Math.max(0, Math.floor(top / rowH) - 4));
  const last = $derived(Math.min(rows, Math.ceil((top + height) / rowH) + 4));
  const visible = $derived(shown.slice(first * cols, last * cols));

  // ---- which tile pictures are fetched first: the ones in view, then the ones about to be scrolled to ----
  const queue = new ImageQueue();
  let lastTop = 0;
  let heading = 1; // 1 scrolling down, -1 up
  function onScroll(e: Event) {
    const t = (e.currentTarget as HTMLElement).scrollTop;
    if (t !== lastTop) heading = t > lastTop ? 1 : -1;
    lastTop = t;
    top = t;
  }
  /** Rows between a tile and the screen: 0 on it, then growing; the side the user is heading to counts as nearer. */
  function distance(idx: number): number {
    const row = Math.floor(idx / cols);
    const above = top / rowH - row;
    const below = row - (top + height) / rowH;
    if (above <= 0 && below <= 0) return (idx % cols) / 1000 + row / 1e6; // in view: reading order
    return above > 0 ? above + (heading > 0 ? 0.5 : 0) : below + (heading < 0 ? 0.5 : 0);
  }
  /** Loads the tile's picture when the queue's turn comes, instead of the moment it is added to the page. */
  function queued(img: HTMLImageElement, p: { url: string; idx: number; slow: boolean }) {
    let now = p;
    let finished = false;
    const done = queue.add({
      rank: () => distance(now.idx),
      slow: p.slow,
      start: () => {
        img.src = now.url;
      },
    });
    const end = () => {
      finished = true;
      done();
    };
    img.addEventListener('load', end);
    img.addEventListener('error', end);
    return {
      update(next: typeof p) {
        now = next;
      },
      destroy() {
        img.removeEventListener('load', end);
        img.removeEventListener('error', end);
        if (!finished) img.removeAttribute('src'); // stop a fetch nobody is waiting for any more
        done();
      },
    };
  }

  $effect(() => {
    if (!scroller) return;
    const el = scroller;
    const measure = () => {
      width = el.clientWidth;
      height = el.clientHeight;
    };
    measure();
    const ro = new ResizeObserver(measure);
    ro.observe(el);
    return () => ro.disconnect();
  });

  // a change of filter starts again from the top
  $effect(() => {
    scope;
    kind;
    newestFirst;
    if (scroller) scroller.scrollTop = 0;
  });

  // ---- the group a file is in, opened by clicking its marker or from the full-size view ----
  let panel = $state<Group | null>(null);
  const openPanel = (f: { relative_path: string }) => {
    const g = review.groupsFor(f.relative_path)[0];
    if (g) {
      viewing = null;
      panel = g;
    }
  };
  // The group may be regrouped under the panel (a decision never does that, a re-scan can): keep what is shown current.
  const panelGroup = $derived(panel ? (review.groups.find((g) => g.id === panel!.id) ?? null) : null);

  // Escape closes the panel before anything behind it hears it.
  function onPanelKey(e: KeyboardEvent) {
    if (e.key === 'Escape' && panel && viewing === null) {
      panel = null;
      e.stopImmediatePropagation();
    }
  }

  // ---- looking at one ----
  let viewing = $state<number | null>(null);
  const toDto = (f: LibraryFile): FileDto => ({ ...f, is_placeholder: false });
  const viewFiles = $derived(viewing === null ? [] : shown.map(toDto));

  function toggleMark(e: Event, f: LibraryFile) {
    e.stopPropagation();
    review.markLibrary([f.relative_path], !marked(f));
  }

  const scopes: [Scope, string][] = [
    ['all', 'All'],
    ['grouped', 'In a group'],
    ['alone', 'Not in a group'],
    ['marked', 'Marked'],
  ];
  const kinds: [Kind, string][] = [
    ['all', 'Photos and videos'],
    ['photo', 'Photos'],
    ['video', 'Videos'],
  ];
  const count = (s: Scope) => (s === 'all' ? counts.all : s === 'grouped' ? counts.grouped : s === 'alone' ? counts.all - counts.grouped : counts.marked);
</script>

<svelte:window onkeydowncapture={onPanelKey} />

<section class="library" aria-label="Library">
  <div class="controls rowbar">
    <div class="l">
    <div class="seg" role="group" aria-label="Show">
      {#each scopes as [key, label] (key)}
        <button aria-pressed={scope === key} onclick={() => (scope = key)}>{label}<span class="n">{count(key)}</span></button>
      {/each}
    </div>
    <div class="seg" role="group" aria-label="Kind">
      {#each kinds as [key, label] (key)}
        <button aria-pressed={kind === key} onclick={() => (kind = key)}>{label}</button>
      {/each}
    </div>
    </div>
    <div class="r">
    <span class="hint">{shown.length} shown</span>
    <label class="sort">
      <span class="hint">Sort</span>
      <select class="field" value={newestFirst ? 'newest' : 'oldest'} onchange={(e) => (newestFirst = e.currentTarget.value === 'newest')}>
        <option value="oldest">Oldest first</option>
        <option value="newest">Newest first</option>
      </select>
    </label>
    </div>
  </div>

  {#if error}
    <div class="notice danger" role="alert"><Icon name="info" size={18} /><p>{error}</p></div>
  {:else if !loaded}
    <div class="skeleton" style="height:240px"></div>
  {:else if shown.length === 0}
    <div class="empty">
      <Icon name="tray" size={36} />
      <p class="lead">Nothing here</p>
      <p class="hint">No file matches this filter.</p>
    </div>
  {:else}
    <div class="scroller" bind:this={scroller} onscroll={onScroll}>
      <div class="space" style="height:{rows * rowH}px">
        {#each visible as f, i (f.id)}
          {@const idx = first * cols + i}
          {@const gs = review.groupsFor(f.relative_path)}
          <div
            class="tile"
            class:marked={marked(f)}
            style="left:{(idx % cols) * (size + gap)}px; top:{Math.floor(idx / cols) * rowH}px; width:{size}px; height:{size}px"
          >
            <button class="pic" onclick={() => (viewing = idx)} title="{f.file_name} · {formatBytes(f.size_bytes)}">
              <img use:queued={{ url: api.fileUrl(f.id, 'thumb'), idx, slow: isVideo(f) || f.is_placeholder }} alt={f.file_name} decoding="async" />
            </button>
            {#if isVideo(f)}<span class="badge overlay br" aria-label="Video"><Icon name="film" size={12} /></span>{/if}
            {#if gs.length > 0}
              <button
                class="grp"
                aria-label="In a group of {gs[0].members.length}: open it"
                title="In a group of {gs[0].members.length} ({tabInfo[gs[0].tab].title}). Click to see and decide it."
                onclick={(e) => {
                  e.stopPropagation();
                  openPanel(f);
                }}
              >
                <Icon name="layers" size={14} />{gs[0].members.length}
              </button>
            {/if}
            <button
              class="mark"
              class:on={marked(f)}
              aria-pressed={marked(f)}
              aria-label={marked(f) ? 'Marked for quarantine; click to clear' : 'Mark for quarantine'}
              title={marked(f) ? 'Marked for quarantine (click to clear)' : 'Mark for quarantine'}
              onclick={(e) => toggleMark(e, f)}
            >
              <Icon name="archive" size={16} />
            </button>
          </div>
        {/each}
      </div>
    </div>
  {/if}
</section>

{#if panelGroup}
  <!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
  <div class="scrim" onclick={() => (panel = null)}>
    <div class="panel" role="dialog" aria-modal="true" aria-label="Group" tabindex="-1" onclick={(e) => e.stopPropagation()}>
      <Detail
        {review}
        paused={false}
        group={panelGroup}
        position={1}
        total={1}
        backLabel="Close"
        pager={false}
        onBack={() => (panel = null)}
        onPrev={() => {}}
        onNext={() => {}}
        onAccept={async () => {
          const g = panelGroup;
          panel = null;
          await review.accept([g]);
        }}
      />
      <div class="panel-foot">
        <button class="btn" onclick={() => onOpenGroup(panelGroup)}>Open in the review <Icon name="arrow-right" /></button>
      </div>
    </div>
  </div>
{/if}

{#if viewing !== null}
  <Lightbox
    files={viewFiles}
    index={viewing}
    onIndex={(i) => (viewing = i)}
    onClose={() => (viewing = null)}
    context="Library"
    mark={{ is: (f) => review.decisions[f.relative_path] === false, toggle: (f) => review.markLibrary([f.relative_path], review.decisions[f.relative_path] !== false) }}
    group={{ size: (f) => review.groupsFor(f.relative_path)[0]?.members.length ?? 0, open: openPanel }}
  />
{/if}

<style>
  .library {
    display: grid;
    grid-template-rows: auto minmax(0, 1fr);
    gap: var(--s4);
    height: 100%;
    min-height: 0;
    padding: var(--s5) var(--s6) 0;
  }
  /* once the row cannot hold both groups on a line, the right group's controls sit one over another,
     aligned right (em = the row's font size, so this follows the UI scale) */
  @container (max-width: 67em) {
    .controls .r {
      flex-direction: column;
      align-items: flex-end;
    }
  }
  .sort {
    display: inline-flex;
    align-items: center;
    gap: var(--s2);
  }
  .scroller {
    overflow-y: auto;
    min-height: 0;
    padding-bottom: var(--s6);
  }
  .space {
    position: relative;
  }
  .tile {
    position: absolute;
    border-radius: var(--radius-sm);
    overflow: hidden;
    background: #000;
    box-shadow: var(--shadow-sm);
  }
  .pic {
    display: block;
    width: 100%;
    height: 100%;
    padding: 0;
    border: 0;
    background: transparent;
    cursor: zoom-in;
  }
  .pic img {
    width: 100%;
    height: 100%;
    object-fit: contain;
    display: block;
  }
  .tile.marked .pic {
    opacity: 0.45;
  }
  .br {
    position: absolute;
    bottom: var(--s1);
    right: var(--s1);
  }
  /* quiet until you aim at the tile; always there once a file is marked */
  .mark {
    position: absolute;
    top: var(--s1);
    right: var(--s1);
    display: grid;
    place-items: center;
    width: calc(30px * var(--k));
    height: calc(30px * var(--k));
    padding: 0;
    border: 0;
    border-radius: 999px;
    background: rgb(20 20 22 / 0.7);
    color: #fff;
    cursor: pointer;
    opacity: 0;
    transition: opacity var(--t-fast) var(--ease);
  }
  .tile:hover .mark,
  .mark:focus-visible,
  .mark.on {
    opacity: 1;
  }
  .mark.on {
    background: var(--warning);
    color: #0b0b0c;
  }
  .empty {
    display: grid;
    justify-items: center;
    gap: var(--s2);
    padding: var(--s7) 0;
  }

  .grp {
    position: absolute;
    top: var(--s1);
    left: var(--s1);
    display: inline-flex;
    align-items: center;
    gap: 3px;
    padding: 2px 8px 2px 6px;
    border: 0;
    border-radius: 999px;
    background: var(--accent);
    color: var(--accent-fg);
    font: 600 var(--fs-caption) var(--font);
    cursor: pointer;
  }
  .grp:hover {
    filter: brightness(1.12);
  }
  /* the group opens big: the same cards as in the review, with the same controls */
  .panel {
    width: min(calc(1500px * var(--k)), 96vw);
    max-height: 94vh;
    overflow-y: auto;
    padding: var(--s4);
    border-radius: var(--radius-lg);
    background: var(--bg);
    box-shadow: var(--shadow-lg);
  }
  .panel-foot {
    display: flex;
    justify-content: flex-end;
    padding-top: var(--s3);
  }
</style>
