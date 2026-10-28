<script lang="ts">
  import { api } from './lib/api';
  import { formatBytes, type Group, type Review } from './lib/review.svelte';
  import Lightbox from './Lightbox.svelte';

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

  const dims = (a: { width: number | null; height: number | null } | null) => (a?.width && a?.height ? `${a.width}×${a.height}` : '');
  const minutes = (s: number) => `${Math.floor(s / 60)}:${String(Math.round(s % 60)).padStart(2, '0')}`;
</script>

<svelte:window onkeydown={onKey} />

<section>
  <div class="bar">
    <button class="secondary" onclick={onBack}>← Overview</button>
    <button class="secondary" onclick={onPrev} disabled={position <= 1} title="Previous (←)">‹</button>
    <span>{position} of {total}</span>
    <button class="secondary" onclick={onNext} disabled={position >= total} title="Next (→)">›</button>
    <span class="spacer"></span>
    <span class="hint">{group.label}{group.reasons.length ? ` · ${group.reasons.join(', ').replaceAll('_', ' ')}` : ''}</span>
    {#if isBurst}
      <button class="secondary" onclick={() => review.keepAll(group)} title="Keep every frame in this run">Keep all</button>
    {/if}
    <button onclick={onAccept} title="Accept suggestion and continue (Enter)">
      {status === 'undecided' || status === 'partial' ? (isBurst ? 'Keep suggested only' : 'Accept suggestion') : 'Next'} ⏎
    </button>
  </div>

  <div class="members">
    {#each group.members as m, i (m.id)}
      {@const keep = review.effective(group, m)}
      {@const decided = review.isDecided(m)}
      <article class:keep class:active={active === i}>
        <button class="thumb" onclick={() => { active = i; previewing = i; }} title="Large preview (Space)">
          <img src={api.fileUrl(m.id, 'thumb')} alt={m.file_name} loading="lazy" />
          {#if i < 9}<kbd>{i + 1}</kbd>{/if}
        </button>
        <div class="info">
          <strong title={m.relative_path}>{m.file_name}</strong>
          {#await review.analysis(m.id) then a}
            <span>
              {[dims(a), a?.duration_seconds ? minutes(a.duration_seconds) : '', formatBytes(m.size_bytes)].filter(Boolean).join(' · ')}
            </span>
            {#if !isBurst}<span class="hint">{[a?.origin, a?.camera_model].filter(Boolean).join(' · ')}</span>{/if}
          {/await}
          {#if isBurst && group.sharpness[m.id] !== undefined}
            <span class="sharp" title="Sharpness (higher is sharper)">
              <span class="bar-fill" style:width="{sharpest ? (group.sharpness[m.id] / sharpest) * 100 : 0}%"></span>
              <span class="hint">{Math.round(group.sharpness[m.id])}{group.suggested.has(m.id) ? ' · suggested' : ''}</span>
            </span>
          {/if}
        </div>
        <button class="choice" class:keep onclick={() => review.toggle(group, m)} aria-pressed={keep}>
          {keep ? 'Keep' : 'Quarantine'}
        </button>
        {#if !decided && keep && group.suggested.has(m.id) && !isBurst}<span class="hint">suggested</span>{/if}
      </article>
    {/each}
  </div>
  <p class="hint">Quarantined files are moved aside, never deleted — and only when you apply your decisions. Anything marked Keep stays where it is.</p>
</section>

{#if previewing !== null}
  <Lightbox files={group.members} index={previewing} onIndex={(i) => ((previewing = i), (active = i))} onClose={() => (previewing = null)} />
{/if}

<style>
  .bar {
    display: flex;
    align-items: center;
    gap: 0.6rem;
    flex-wrap: wrap;
    margin-bottom: 1rem;
  }
  .spacer {
    flex: 1;
  }
  .members {
    display: grid;
    grid-template-columns: repeat(auto-fit, minmax(15rem, 1fr));
    gap: 1rem;
  }
  article {
    display: flex;
    flex-direction: column;
    gap: 0.5rem;
    padding: 0.6rem;
    border: 1px solid var(--line);
    border-radius: 12px;
    opacity: 0.75;
  }
  article.keep {
    opacity: 1;
    border-color: var(--ok);
  }
  article.active {
    outline: 2px solid var(--accent);
  }
  .thumb {
    position: relative;
    padding: 0;
    background: var(--line);
    border-radius: 8px;
    overflow: hidden;
  }
  .thumb img {
    display: block;
    width: 100%;
    aspect-ratio: 4 / 3;
    object-fit: contain;
  }
  kbd {
    position: absolute;
    top: 6px;
    left: 6px;
    background: rgb(0 0 0 / 0.65);
    color: #fff;
    border-radius: 4px;
    padding: 0 0.4rem;
    font: 0.85em ui-monospace, monospace;
  }
  .info {
    display: flex;
    flex-direction: column;
    font-size: 0.9em;
  }
  .info strong {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .sharp {
    display: flex;
    flex-direction: column;
    gap: 2px;
    margin-top: 2px;
  }
  .bar-fill {
    display: block;
    height: 4px;
    border-radius: 2px;
    background: var(--accent);
    min-width: 2px;
  }
  .choice {
    background: transparent;
    color: inherit;
    border: 1px solid var(--line);
  }
  .choice.keep {
    background: var(--ok);
    color: var(--ok-fg);
    border-color: var(--ok);
  }
</style>
