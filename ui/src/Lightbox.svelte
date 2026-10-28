<script lang="ts">
  import { api, type FileDto } from './lib/api';
  import Icon from './Icon.svelte';

  let { files, index, onIndex, onClose }: { files: FileDto[]; index: number; onIndex: (i: number) => void; onClose: () => void } = $props();

  const file = $derived(files[index]);
  const many = $derived(files.length > 1);
  const go = (delta: number) => onIndex((index + delta + files.length) % files.length);

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
    else if (e.key === 'ArrowRight' && many) go(1);
    else if (e.key === 'ArrowLeft' && many) go(-1);
    e.preventDefault();
    e.stopImmediatePropagation();
  }
</script>

<svelte:window onkeydowncapture={onKey} />

<!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
<div class="lightbox" role="dialog" tabindex="-1" aria-modal="true" aria-label="Preview of {file.file_name}" onclick={onClose}>
  <button class="close btn icon" onclick={onClose} aria-label="Close preview"><Icon name="x" size={18} /></button>

  {#if many}
    <button class="nav prev btn icon" onclick={(e) => { e.stopPropagation(); go(-1); }} aria-label="Previous photo"><Icon name="chevron-left" size={20} /></button>
    <button class="nav next btn icon" onclick={(e) => { e.stopPropagation(); go(1); }} aria-label="Next photo"><Icon name="chevron-right" size={20} /></button>
  {/if}

  {#if file.media_class === 'Video'}
    <!-- svelte-ignore a11y_media_has_caption -->
    <video src={api.fileUrl(file.id, 'original')} controls autoplay onclick={(e) => e.stopPropagation()}></video>
  {:else}
    <!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
    <img src={api.fileUrl(file.id, 'preview')} alt={file.file_name} onclick={(e) => e.stopPropagation()} />
  {/if}

  <span class="caption">
    {file.file_name}{many ? ` · ${index + 1} of ${files.length}` : ''}
    <span class="keys"><kbd>Esc</kbd> to close{many ? ' · ← → to switch' : ''}</span>
  </span>
</div>

<style>
  .lightbox {
    position: fixed;
    inset: 0;
    z-index: 40;
    display: grid;
    place-items: center;
    padding: var(--s6) var(--s7);
    background: rgb(8 8 10 / 0.9);
    backdrop-filter: blur(6px);
    animation: fade var(--t) var(--ease);
  }
  img,
  video {
    max-width: 100%;
    max-height: calc(100vh - 120px);
    border-radius: var(--radius-sm);
    box-shadow: 0 20px 60px rgb(0 0 0 / 0.6);
  }
  .close {
    position: absolute;
    top: var(--s4);
    right: var(--s4);
  }
  .nav {
    position: absolute;
    top: 50%;
    translate: 0 -50%;
    width: 40px;
    min-height: 40px;
    border-radius: 50%;
  }
  .prev {
    left: var(--s4);
  }
  .next {
    right: var(--s4);
  }
  .caption {
    position: absolute;
    bottom: var(--s4);
    display: inline-flex;
    gap: var(--s3);
    align-items: center;
    max-width: calc(100% - 2 * var(--s4));
    padding: 6px var(--s3);
    border-radius: 999px;
    background: rgb(40 40 44 / 0.78);
    color: #f5f5f7;
    font-size: var(--fs-small);
    backdrop-filter: blur(10px);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .keys {
    color: #b8b8c0;
  }
  @media (max-width: 640px) {
    .lightbox {
      padding: var(--s6) var(--s3);
    }
    .keys {
      display: none;
    }
  }
</style>
