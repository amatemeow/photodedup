<script lang="ts">
  import { onMount } from 'svelte';
  import { api } from './lib/api';
  import { formatWhen, type Review } from './lib/review.svelte';
  import Icon from './Icon.svelte';

  let {
    review,
    focusId,
    onOpen,
    onFocus,
  }: { review: Review; focusId: string | null; onOpen: (id: string) => void; onFocus: (id: string) => void } = $props();

  let grid: HTMLElement | undefined = $state();

  // Coming back from a group: put keyboard focus back on its tile.
  onMount(() => {
    if (focusId !== null) grid?.querySelector<HTMLElement>(`[data-id="${focusId}"]`)?.focus({ preventScroll: false });
  });

  function columns(): number {
    return grid ? Math.max(1, getComputedStyle(grid).gridTemplateColumns.split(' ').length) : 1;
  }

  // Arrow keys move the focus ring between tiles; Enter (a real button press)
  // opens it. Handled here, not in the shell, because only the overview knows
  // its column count.
  function onKey(e: KeyboardEvent) {
    if (e.metaKey || e.ctrlKey || e.altKey) return;
    const ids = review.visible.map((c) => c.id);
    const pos = focusId === null ? -1 : ids.indexOf(focusId);
    const delta = { ArrowRight: 1, ArrowLeft: -1, ArrowDown: columns(), ArrowUp: -columns() }[e.key];
    if (delta === undefined) return;
    e.preventDefault();
    const next = ids[Math.min(Math.max(pos + delta, 0), ids.length - 1)];
    if (next !== undefined) {
      onFocus(next);
      grid?.querySelector<HTMLElement>(`[data-id="${next}"]`)?.focus();
    }
  }

  // A photo fades in once it has loaded, so the grid never flashes empty boxes.
  const loaded = (e: Event) => (e.currentTarget as HTMLImageElement).classList.add('in');
  const ready = (node: HTMLImageElement) => {
    if (node.complete) node.classList.add('in');
  };
</script>

<!-- svelte-ignore a11y_no_static_element_interactions -->
<div class="grid" bind:this={grid} onkeydown={onKey}>
  {#each review.visible as c (c.id)}
    {@const status = review.statuses.get(c.id) ?? 'undecided'}
    {@const shown = c.members.find((m) => review.effective(c, m)) ?? c.members[0]}
    {@const kept = c.members.filter((m) => review.effective(c, m)).length}
    <button
      class="tile {status}"
      data-id={c.id}
      onclick={() => onOpen(c.id)}
      onfocus={() => onFocus(c.id)}
    >
      <span class="thumb" class:pair={c.tab === 'similar'}>
        {#if c.tab === 'similar'}
          {#each c.members as m (m.id)}
            <img src={api.fileUrl(m.id, 'thumb')} alt="" loading="lazy" decoding="async" onload={loaded} use:ready />
          {/each}
        {:else}
          <img src={api.fileUrl(shown.id, 'thumb')} alt="" loading="lazy" decoding="async" onload={loaded} use:ready />
        {/if}

        {#if status === 'accepted'}
          <span class="badge overlay success tl"><Icon name="check" size={11} /> Accepted</span>
        {:else if status === 'changed'}
          <span class="badge overlay accent tl"><Icon name="pencil" size={11} /> Edited</span>
        {:else if status === 'partial'}
          <span class="badge overlay warning tl">Partly decided</span>
        {/if}
        <span class="badge overlay br">{c.tab === 'burst' ? `${kept} of ${c.members.length} kept` : `${c.members.length} ${c.members.length === 1 ? 'file' : 'files'}`}</span>
      </span>
      <span class="meta">
        <span class="date">{formatWhen(c.capturedAt)}</span>
        <span class="sub">{c.label}</span>
      </span>
    </button>
  {/each}
</div>

<style>
  .grid {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(220px, 1fr));
    gap: var(--s4);
  }
  .tile {
    display: grid;
    gap: var(--s2);
    padding: 0;
    border: 0;
    background: transparent;
    color: inherit;
    text-align: left;
    cursor: pointer;
    border-radius: var(--radius);
    /* long libraries stay fast: off-screen tiles are not laid out or painted */
    content-visibility: auto;
    contain-intrinsic-size: 220px 250px;
  }
  .thumb {
    position: relative;
    display: block;
    aspect-ratio: 4 / 3;
    overflow: hidden;
    border-radius: var(--radius);
    background: var(--surface-2);
    box-shadow: var(--shadow-sm);
    transition: transform var(--t) var(--ease), box-shadow var(--t) var(--ease);
  }
  .thumb.pair {
    display: grid;
    grid-template-columns: 1fr 1fr;
    gap: 2px;
  }
  .tile:hover .thumb {
    transform: translateY(-2px);
    box-shadow: 0 6px 18px rgb(0 0 0 / 0.18), 0 0 0 1px var(--separator);
  }
  .tile:focus-visible .thumb {
    box-shadow: var(--ring);
  }
  .tile:active .thumb {
    transform: scale(0.99);
  }
  img {
    width: 100%;
    height: 100%;
    object-fit: cover;
    display: block;
    opacity: 0;
    transition: opacity 0.25s var(--ease);
  }
  img:global(.in) {
    opacity: 1;
  }
  .tl {
    position: absolute;
    top: var(--s2);
    left: var(--s2);
  }
  .br {
    position: absolute;
    bottom: var(--s2);
    right: var(--s2);
  }
  .meta {
    display: grid;
    gap: 1px;
    padding: 0 2px;
  }
  .date {
    font-weight: 600;
  }
  .sub {
    color: var(--label-3);
    font-size: var(--fs-small);
  }
</style>
