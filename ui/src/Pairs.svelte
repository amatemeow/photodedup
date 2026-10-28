<script lang="ts">
  import { api, type FileDto, type ReviewPair } from './lib/api';
  import Lightbox from './Lightbox.svelte';
  import { formatBytes } from './lib/review.svelte';

  let { pairs }: { pairs: ReviewPair[] } = $props();

  let open = $state<{ files: FileDto[]; index: number } | null>(null);

  const reasonText = (r: string) => r.replaceAll('_', ' ');
</script>

{#if pairs.length === 0}
  <p class="hint">No same-device look-alikes were found.</p>
{:else}
  <div class="grid">
    {#each pairs as p (p.id)}
      {@const files = [p.file_a, p.file_b].filter((f): f is FileDto => f !== null)}
      <div class="pair">
        <div class="thumbs">
          {#each files as f, i (f.id)}
            <button onclick={() => (open = { files, index: i })} title="Large preview">
              <img src={api.fileUrl(f.id, 'thumb')} alt={f.file_name} loading="lazy" />
            </button>
          {/each}
        </div>
        <div class="names">
          {#each files as f (f.id)}<span title={f.relative_path}>{f.file_name} · {formatBytes(f.size_bytes)}</span>{/each}
        </div>
        <span class="hint">{reasonText(p.reason)} · difference {p.phash_dist}</span>
      </div>
    {/each}
  </div>
{/if}

{#if open}
  <Lightbox files={open.files} index={open.index} onIndex={(i) => open && (open = { ...open, index: i })} onClose={() => (open = null)} />
{/if}

<style>
  .grid {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(20rem, 1fr));
    gap: 0.75rem;
  }
  .pair {
    display: flex;
    flex-direction: column;
    gap: 0.35rem;
    padding: 0.5rem;
    border: 1px solid var(--line);
    border-radius: 10px;
  }
  .thumbs {
    display: grid;
    grid-template-columns: 1fr 1fr;
    gap: 0.4rem;
  }
  .thumbs button {
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
  .names {
    display: grid;
    grid-template-columns: 1fr 1fr;
    gap: 0.4rem;
    font-size: 0.8em;
  }
  .names span {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
</style>
