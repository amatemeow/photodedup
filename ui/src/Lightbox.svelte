<script lang="ts">
  import { api, type FileDto } from './lib/api';

  let { files, index, onIndex, onClose }: { files: FileDto[]; index: number; onIndex: (i: number) => void; onClose: () => void } = $props();

  const file = $derived(files[index]);

  // Capture phase and stopImmediatePropagation: while a preview is open it
  // owns the keyboard, so nothing behind it (Enter = accept, Cmd+Z = undo,
  // number keys = toggle) can act on something the user can't see.
  function onKey(e: KeyboardEvent) {
    if (e.metaKey || e.ctrlKey || e.altKey) {
      e.stopImmediatePropagation();
      return;
    }
    if (e.key === 'Tab') return;
    if (e.key === 'Escape' || e.key === ' ') onClose();
    else if (e.key === 'ArrowRight') onIndex((index + 1) % files.length);
    else if (e.key === 'ArrowLeft') onIndex((index - 1 + files.length) % files.length);
    e.preventDefault();
    e.stopImmediatePropagation();
  }
</script>

<svelte:window onkeydowncapture={onKey} />

<!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
<div class="lightbox" onclick={onClose}>
  {#if file.media_class === 'Video'}
    <!-- svelte-ignore a11y_media_has_caption -->
    <video src={api.fileUrl(file.id, 'original')} controls autoplay onclick={(e) => e.stopPropagation()}></video>
  {:else}
    <img src={api.fileUrl(file.id, 'preview')} alt={file.file_name} />
  {/if}
  <span class="caption">{file.file_name}{files.length > 1 ? ' · ← → switch' : ''} · Esc close</span>
</div>

<style>
  .lightbox {
    position: fixed;
    inset: 0;
    background: rgb(0 0 0 / 0.88);
    display: grid;
    place-items: center;
    z-index: 20;
  }
  img,
  video {
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
