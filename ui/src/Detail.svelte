<script lang="ts">
  import { api, type Cluster, type FileDto } from './lib/api';
  import { formatBytes, tierLabel, type Review } from './lib/review.svelte';

  let {
    review,
    paused,
    cluster,
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
    cluster: Cluster;
    position: number;
    total: number;
    onBack: () => void;
    onPrev: () => void;
    onNext: () => void;
    onAccept: () => void;
  } = $props();

  let active = $state(0); // highlighted member: target of Space
  let previewing = $state<FileDto | null>(null);

  // A new cluster starts with the first member highlighted and no preview.
  $effect(() => {
    cluster.id;
    active = 0;
    previewing = null;
  });

  const status = $derived(review.statuses.get(cluster.id) ?? 'undecided');

  function onKey(e: KeyboardEvent) {
    if (paused || e.metaKey || e.ctrlKey || e.altKey) return;
    const members = cluster.members;
    if (previewing) {
      // Preview captures the keys; the shell's navigation must not also fire.
      const i = members.findIndex((m) => m.id === previewing!.id);
      if (e.key === 'Escape' || e.key === ' ') previewing = null;
      else if (e.key === 'ArrowRight') previewing = members[(i + 1) % members.length];
      else if (e.key === 'ArrowLeft') previewing = members[(i - 1 + members.length) % members.length];
      // Any other key does nothing, but must not reach the shell either
      // (Enter would accept and advance behind the preview).
      else if (e.key === '?' || e.key === 'Tab') return;
      e.preventDefault();
      e.stopImmediatePropagation();
      return;
    }
    if (/^[1-9]$/.test(e.key)) {
      const m = members[Number(e.key) - 1];
      if (m) {
        active = Number(e.key) - 1;
        review.toggle(cluster, m);
        e.preventDefault();
      }
    } else if (e.key === ' ' && (e.target as HTMLElement).tagName !== 'BUTTON') {
      previewing = members[active] ?? null;
      e.preventDefault();
    }
  }

  const dims = (a: { width: number | null; height: number | null } | null) => (a?.width && a?.height ? `${a.width}×${a.height}` : '');
  const minutes = (s: number) => `${Math.floor(s / 60)}:${String(Math.round(s % 60)).padStart(2, '0')}`;
</script>

<!-- Capture phase, so an open preview can swallow keys before the shell sees them. -->
<svelte:window onkeydowncapture={onKey} />

<section>
  <div class="bar">
    <button class="secondary" onclick={onBack}>← Overview</button>
    <button class="secondary" onclick={onPrev} disabled={position <= 1} title="Previous (←)">‹</button>
    <span>{position} of {total}</span>
    <button class="secondary" onclick={onNext} disabled={position >= total} title="Next (→)">›</button>
    <span class="spacer"></span>
    <span class="hint">{tierLabel[cluster.tier] ?? cluster.tier}{cluster.reasons.length ? ` · ${cluster.reasons.join(', ').replaceAll('_', ' ')}` : ''}</span>
    <button onclick={onAccept} title="Accept suggestion and continue (Enter)">
      {status === 'undecided' || status === 'partial' ? 'Accept suggestion' : 'Next'} ⏎
    </button>
  </div>

  <div class="members">
    {#each cluster.members as m, i (m.id)}
      {@const keep = review.effective(cluster, m)}
      {@const decided = review.isDecided(m)}
      <article class:keep class:active={active === i}>
        <button class="thumb" onclick={() => { active = i; previewing = m; }} title="Large preview (Space)">
          <img src={api.fileUrl(m.id, 'thumb')} alt={m.file_name} />
          <kbd>{i + 1}</kbd>
        </button>
        <div class="info">
          <strong title={m.relative_path}>{m.file_name}</strong>
          {#await review.analysis(m.id) then a}
            <span>
              {[dims(a), a?.duration_seconds ? minutes(a.duration_seconds) : '', formatBytes(m.size_bytes)].filter(Boolean).join(' · ')}
            </span>
            <span class="hint">{[a?.origin, a?.camera_model].filter(Boolean).join(' · ')}</span>
          {/await}
        </div>
        <button class="choice" class:keep onclick={() => review.toggle(cluster, m)} aria-pressed={keep}>
          {keep ? 'Keep' : 'Quarantine'}
        </button>
        {#if !decided && keep && m.id === cluster.keeper_file_id}<span class="hint">suggested</span>{/if}
      </article>
    {/each}
  </div>
  <p class="hint">Quarantined files are moved aside, never deleted — and only when you apply your decisions.</p>
</section>

{#if previewing}
  <!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
  <div class="lightbox" onclick={() => (previewing = null)}>
    {#if previewing.media_class === 'Video'}
      <!-- svelte-ignore a11y_media_has_caption -->
      <video src={api.fileUrl(previewing.id, 'original')} controls autoplay onclick={(e) => e.stopPropagation()}></video>
    {:else}
      <img src={api.fileUrl(previewing.id, 'preview')} alt={previewing.file_name} />
    {/if}
    <span class="caption">{previewing.file_name} · ← → switch · Esc close</span>
  </div>
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
  .lightbox {
    position: fixed;
    inset: 0;
    background: rgb(0 0 0 / 0.88);
    display: grid;
    place-items: center;
    z-index: 20;
  }
  .lightbox img,
  .lightbox video {
    max-width: 94vw;
    max-height: 90vh;
  }
  .caption {
    position: absolute;
    bottom: 1rem;
    color: #ddd;
    font-size: 0.85em;
  }
</style>
