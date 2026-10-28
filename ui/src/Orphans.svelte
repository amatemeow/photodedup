<script lang="ts">
  import { api, type OrphanVideo } from './lib/api';
  import Lightbox from './Lightbox.svelte';
  import { formatBytes } from './lib/review.svelte';

  let { orphans }: { orphans: OrphanVideo[] } = $props();

  let open = $state<number | null>(null);
  const files = $derived(orphans.map((o) => o.file));

  const sections = $derived([
    {
      reason: 'no_match',
      title: 'No original found',
      note: 'These look like copies (re-encoded, forwarded) but none of your camera videos matches them.',
      items: orphans.filter((o) => o.reason === 'no_match'),
    },
    {
      reason: 'unknown_origin',
      title: 'Source not identified',
      note: "The scan couldn't establish where these came from, so it never compared them against anything.",
      items: orphans.filter((o) => o.reason === 'unknown_origin'),
    },
  ]);
</script>

{#snippet card(o: OrphanVideo, i: number)}
  <div class="card">
    <button onclick={() => (open = i)} title="Play">
      <img src={api.fileUrl(o.file.id, 'thumb')} alt={o.file.file_name} loading="lazy" />
    </button>
    <strong title={o.file.relative_path}>{o.file.file_name}</strong>
    <span class="hint">{formatBytes(o.file.size_bytes)}</span>
  </div>
{/snippet}

{#if orphans.length === 0}
  <p class="hint">Every copy-like video found its original.</p>
{:else}
  {#each sections as sec (sec.reason)}
    {#if sec.items.length}
      <h3>{sec.title} <span class="hint">{sec.items.length}</span></h3>
      <p class="hint">{sec.note}</p>
      <div class="grid">
        {#each sec.items as o (o.file.id)}{@render card(o, orphans.indexOf(o))}{/each}
      </div>
    {/if}
  {/each}
{/if}

{#if open !== null}
  <Lightbox {files} index={open} onIndex={(i) => (open = i)} onClose={() => (open = null)} />
{/if}

<style>
  .grid {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(14rem, 1fr));
    gap: 0.75rem;
  }
  h3 {
    font-size: 1rem;
    margin: 1.25rem 0 0.25rem;
  }
  .card {
    display: flex;
    flex-direction: column;
    gap: 0.3rem;
    padding: 0.5rem;
    border: 1px solid var(--line);
    border-radius: 10px;
  }
  .card button {
    padding: 0;
    background: var(--line);
    border-radius: 6px;
    overflow: hidden;
  }
  img {
    display: block;
    width: 100%;
    aspect-ratio: 4 / 3;
    object-fit: cover;
  }
  strong {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    font-size: 0.9em;
  }
</style>
