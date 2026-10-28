<script lang="ts">
  import { onMount } from 'svelte';
  import { api } from './lib/api';
  import { tierLabel, type Review } from './lib/review.svelte';

  let {
    review,
    focusId,
    onOpen,
    onFocus,
  }: { review: Review; focusId: string | null; onOpen: (id: string) => void; onFocus: (id: string) => void } = $props();

  let grid: HTMLElement | undefined = $state();

  // Coming back from a cluster: put keyboard focus back on its card.
  onMount(() => {
    if (focusId !== null) grid?.querySelector<HTMLElement>(`[data-id="${focusId}"]`)?.focus();
  });

  const badge = { undecided: 'Undecided', partial: 'Partly decided', accepted: 'Accepted', changed: 'Changed' } as const;

  function columns(): number {
    return grid ? Math.max(1, getComputedStyle(grid).gridTemplateColumns.split(' ').length) : 1;
  }

  // Arrow keys move the focus ring between cards; Enter (a real button
  // press) opens it. Handled here, not in the shell, because only the
  // overview knows its column count.
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
</script>

<!-- svelte-ignore a11y_no_static_element_interactions -->
<div class="grid" bind:this={grid} onkeydown={onKey}>
  {#each review.visible as c (c.id)}
    {@const status = review.statuses.get(c.id) ?? 'undecided'}
    {@const shown = c.members.find((m) => review.effective(c, m)) ?? c.members[0]}
    <button class="card {status}" data-id={c.id} class:focus={focusId === c.id} onclick={() => onOpen(c.id)} onfocus={() => onFocus(c.id)}>
      <img src={api.fileUrl(shown.id, 'thumb')} alt={shown.file_name} loading="lazy" />
      <span class="meta">
        <span class="badge {status}">{badge[status]}</span>
        <span>{c.members.length} files</span>
      </span>
      <span class="tier">{tierLabel[c.tier] ?? c.tier}</span>
    </button>
  {/each}
</div>

<style>
  .grid {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(11rem, 1fr));
    gap: 0.75rem;
  }
  .card {
    display: flex;
    flex-direction: column;
    gap: 0.35rem;
    padding: 0.5rem;
    text-align: left;
    background: transparent;
    color: inherit;
    border: 1px solid var(--line);
    border-radius: 10px;
  }
  .card:focus-visible,
  .card.focus {
    outline: 2px solid var(--accent);
    outline-offset: 1px;
  }
  .card img {
    width: 100%;
    aspect-ratio: 4 / 3;
    object-fit: cover;
    border-radius: 6px;
    background: var(--line);
  }
  .meta {
    display: flex;
    justify-content: space-between;
    font-size: 0.85em;
    color: var(--muted);
  }
  .tier {
    font-size: 0.8em;
    color: var(--muted);
  }
  .badge {
    font-weight: 600;
  }
  .badge.accepted {
    color: var(--ok);
  }
  .badge.changed {
    color: var(--accent);
  }
  .badge.partial {
    color: var(--danger);
  }
</style>
