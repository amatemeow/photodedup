<script lang="ts">
  import { api, type FileDto } from './lib/api';
  import Icon from './Icon.svelte';

  let {
    files,
    index,
    onIndex,
    onClose,
    context = '',
    urls,
    mark,
    group,
  }: {
    files: FileDto[];
    index: number;
    onIndex: (i: number) => void;
    onClose: () => void;
    /** Where this group is, shown quietly in the caption ("Group 12 of 87 · Exact duplicates"). */
    context?: string;
    /** Where the pictures come from when the files are not the scan's own (files in quarantine). */
    urls?: (f: FileDto) => { low: string; high: string };
    /** Lets the person discard what they are looking at: a Quarantine toggle on the picture. */
    mark?: { is: (f: FileDto) => boolean; toggle: (f: FileDto) => void };
    /** For a file that is in a group of look-alikes: a button that opens the group. */
    group?: { size: (f: FileDto) => number; open: (f: FileDto) => void };
  } = $props();

  const file = $derived(files[index]);
  const many = $derived(files.length > 1);
  // A video plays from its original; one in quarantine shows its first frame instead.
  const playsVideo = $derived(file.media_class === 'Video' && !urls);
  const go = (delta: number) => onIndex((index + delta + files.length) % files.length);

  // The picture shown is the original file itself wherever a browser can display it, so
  // zooming reaches the real pixels. Formats it cannot display (HEIC, RAW, TIFF...) are
  // decoded by the server at their full size instead. The default preview, already in the
  // cache from the detail view, shows at once underneath while the full one loads.
  const showsItself = (f: FileDto) => /\.(jpe?g|png|gif|webp|bmp|avif)$/i.test(f.file_name);
  const lowUrl = (f: FileDto) => (urls ? urls(f).low : api.fileUrl(f.id, 'preview'));
  const fullUrl = (f: FileDto) => (urls ? urls(f).high : showsItself(f) ? api.fileUrl(f.id, 'original') : api.fileUrl(f.id, 'preview', 16384));

  // Zoom: 1 is "fit the window"; the view is moved by `tx`/`ty` (px from centre).
  const MAX_ZOOM = 16;
  let scale = $state(1);
  let tx = $state(0);
  let ty = $state(0);
  let stage: HTMLElement | undefined = $state();
  let img: HTMLImageElement | undefined = $state(); // the fitted default preview: same proportions as the sharp one
  let hi: HTMLImageElement | undefined = $state();
  let dragging = $state(false);
  let moved = false;

  // A new picture starts fitted.
  $effect(() => {
    file.id;
    reset();
  });

  function reset() {
    scale = 1;
    tx = 0;
    ty = 0;
  }

  /** The size the picture is drawn at when fitted, in CSS px. */
  function fitted(): { w: number; h: number; sw: number; sh: number } | null {
    if (!stage || !img || !img.naturalWidth) return null;
    const sw = stage.clientWidth;
    const sh = stage.clientHeight;
    const f = Math.min(sw / img.naturalWidth, sh / img.naturalHeight);
    return { w: img.naturalWidth * f, h: img.naturalHeight * f, sw, sh };
  }

  /** The picture never leaves the window: when zoomed it can be moved only as far as its edges. */
  function clampPan() {
    const f = fitted();
    if (!f) return;
    const mx = Math.max(0, (f.w * scale - f.sw) / 2);
    const my = Math.max(0, (f.h * scale - f.sh) / 2);
    tx = Math.min(mx, Math.max(-mx, tx));
    ty = Math.min(my, Math.max(-my, ty));
  }

  /** Zooms to `next`, keeping the point under (cx, cy) -- px from the window's centre -- where it is. */
  function zoomTo(next: number, cx = 0, cy = 0) {
    next = Math.min(MAX_ZOOM, Math.max(1, next));
    const r = next / scale;
    tx = cx - (cx - tx) * r;
    ty = cy - (cy - ty) * r;
    scale = next;
    clampPan();
  }

  const fromCentre = (e: { clientX: number; clientY: number }) => {
    const r = stage!.getBoundingClientRect();
    return { x: e.clientX - r.left - r.width / 2, y: e.clientY - r.top - r.height / 2 };
  };

  function onWheel(e: WheelEvent) {
    if (playsVideo) return;
    e.preventDefault();
    const c = fromCentre(e);
    zoomTo(scale * Math.exp(-e.deltaY * (e.deltaMode === 1 ? 0.05 : 0.0016)), c.x, c.y);
  }

  function onDouble(e: MouseEvent) {
    const f = fitted();
    if (!f || !img) return;
    if (scale > 1) return reset();
    const c = fromCentre(e);
    // 100% (one image pixel per screen pixel) if that is a zoom at all, else 2.5x
    const actual = ((hi && sharp ? hi : img).naturalWidth / f.w) / (window.devicePixelRatio || 1);
    zoomTo(actual > 1.4 ? actual : 2.5, c.x, c.y);
  }

  let last = { x: 0, y: 0 };
  function down(e: PointerEvent) {
    if (scale <= 1 || e.button !== 0) return;
    dragging = true;
    moved = false;
    last = { x: e.clientX, y: e.clientY };
    (e.currentTarget as HTMLElement).setPointerCapture(e.pointerId);
  }
  function drag(e: PointerEvent) {
    if (!dragging) return;
    tx += e.clientX - last.x;
    ty += e.clientY - last.y;
    if (Math.abs(e.clientX - last.x) + Math.abs(e.clientY - last.y) > 2) moved = true;
    last = { x: e.clientX, y: e.clientY };
    clampPan();
  }
  const up = () => (dragging = false);

  // Clicking anywhere but on the picture closes the view -- unless that click ended a drag.
  // The picture's box is where it is drawn: fitted, then zoomed and moved.
  function stageClick(e: MouseEvent) {
    const was = moved;
    moved = false;
    if (was) return;
    const f = fitted();
    if (!f || playsVideo) return void (e.target === stage && onClose());
    const c = fromCentre(e);
    const halfW = (f.w * scale) / 2;
    const halfH = (f.h * scale) / 2;
    const onPicture = Math.abs(c.x - tx) <= halfW && Math.abs(c.y - ty) <= halfH;
    if (!onPicture) onClose();
  }

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
    else if (e.key === '+' || e.key === '=') zoomTo(scale * 1.4);
    else if (e.key === '-' || e.key === '_') zoomTo(scale / 1.4);
    else if (e.key === '0') reset();
    else if ((e.key === 'q' || e.key === 'Q') && mark) mark.toggle(file);
    else if ((e.key === 'g' || e.key === 'G') && group && group.size(file) > 0) group.open(file);
    e.preventDefault();
    e.stopImmediatePropagation();
  }

  let sharp = $state(false);
  // The full-size picture can fail (a file a browser will not show, a slow disk): the original is
  // then asked of the server as a full-size decode instead, and if that fails too, it is said.
  let viaServer = $state(false);
  let failed = $state(false);
  const failedLoad = () => {
    if (!urls && !viaServer && showsItself(file)) viaServer = true;
    else failed = true;
  };
  $effect(() => {
    file.id;
    sharp = false;
    viaServer = false;
    failed = false;
  });
</script>

<svelte:window onkeydowncapture={onKey} />

<!-- svelte-ignore a11y_click_events_have_key_events -->
<div
  class="lightbox"
  role="dialog"
  tabindex="-1"
  aria-modal="true"
  aria-label="Preview of {file.file_name}"
  onclick={(e) => e.target === e.currentTarget && onClose()}
>
  {#if many}
    <div class="heading" aria-live="polite">Photo {index + 1} of {files.length}</div>
  {/if}
  <button class="close btn icon" onclick={onClose} aria-label="Close preview"><Icon name="x" size={18} /></button>

  {#if many}
    <button class="nav prev btn icon" onclick={() => go(-1)} aria-label="Previous photo"><Icon name="chevron-left" size={20} /></button>
    <button class="nav next btn icon" onclick={() => go(1)} aria-label="Next photo"><Icon name="chevron-right" size={20} /></button>
  {/if}

  <!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
  <div
    class="stage"
    class:zoomed={scale > 1}
    class:dragging
    bind:this={stage}
    onclick={stageClick}
    onwheel={onWheel}
    ondblclick={onDouble}
    onpointerdown={down}
    onpointermove={drag}
    onpointerup={up}
    onpointercancel={up}
  >
    {#if playsVideo}
      <!-- svelte-ignore a11y_media_has_caption -->
      <video src={api.fileUrl(file.id, 'original')} controls autoplay></video>
    {:else}
      <div class="pic" style="transform: translate({tx}px, {ty}px) scale({scale})">
        <img class="low" src={lowUrl(file)} alt="" aria-hidden="true" draggable="false" bind:this={img} />
        <img
          class="high"
          class:in={sharp}
          src={viaServer ? api.fileUrl(file.id, 'preview', 16384) : fullUrl(file)}
          alt={file.file_name}
          draggable="false"
          bind:this={hi}
          onload={() => (sharp = true)}
          onerror={failedLoad}
        />
      </div>
    {/if}
  </div>

  {#if mark || group}
    <div class="acts">
      {#if mark}
        {@const on = mark.is(file)}
        <button class="btn act" class:on aria-pressed={on} onclick={() => mark.toggle(file)} title="Mark this one for quarantine (Q)">
          <Icon name="archive" /> {on ? 'Marked for quarantine' : 'Quarantine'} <span class="k">Q</span>
        </button>
      {/if}
      {#if group && group.size(file) > 0}
        <button class="btn act" onclick={() => group.open(file)} title="See and decide the group this is in (G)">
          <Icon name="layers" /> Group of {group.size(file)} <span class="k">G</span>
        </button>
      {/if}
    </div>
  {/if}

  <span class="caption">
    {file.file_name}{context ? ` · ${context}` : ''}
    {#if !playsVideo}
      <span class="zoom">{Math.round(scale * 100)}%</span>
      {#if failed}
        <span class="res warn">full size unavailable: showing a smaller copy</span>
      {:else if !sharp}
        <span class="res"><span class="spinner"></span> loading full size…</span>
      {:else if hi}
        <span class="res" title="The size of the picture you are looking at">{hi.naturalWidth} × {hi.naturalHeight}</span>
      {/if}
    {/if}
    <span class="keys"><kbd>Esc</kbd> close{many ? ' · ← → switch' : ''}{!playsVideo ? ' · scroll or + − zoom · drag to move · double-click 100% · 0 fit' : ''}</span>
  </span>
</div>

<style>
  .lightbox {
    position: fixed;
    inset: 0;
    z-index: 40;
    display: grid;
    background: rgb(8 8 10 / 0.94);
    backdrop-filter: blur(6px);
    animation: fade var(--t) var(--ease);
  }
  /* the picture gets all the room there is, under the heading and above the caption */
  .stage {
    position: absolute;
    inset: calc(var(--s6) + var(--s5)) var(--s7) calc(var(--s6) + var(--s5)) var(--s7);
    overflow: hidden;
    touch-action: none;
  }
  .stage.zoomed {
    cursor: grab;
  }
  .stage.dragging {
    cursor: grabbing;
  }
  .pic {
    width: 100%;
    height: 100%;
    position: relative;
    transform-origin: center;
    will-change: transform;
  }
  .pic img {
    position: absolute;
    inset: 0;
    width: 100%;
    height: 100%;
    object-fit: contain;
    user-select: none;
  }
  .pic img.high {
    opacity: 0;
    transition: opacity 0.25s var(--ease);
  }
  .pic img.high.in {
    opacity: 1;
  }
  video {
    width: 100%;
    height: 100%;
    object-fit: contain;
    background: #000;
  }
  .heading {
    position: absolute;
    top: var(--s4);
    left: 50%;
    translate: -50% 0;
    z-index: 1;
    padding: var(--s1) var(--s4);
    border-radius: 999px;
    background: rgb(40 40 44 / 0.78);
    color: #f5f5f7;
    font-size: var(--fs-lead);
    font-weight: 600;
    font-variant-numeric: tabular-nums;
    backdrop-filter: blur(10px);
    white-space: nowrap;
  }
  .close {
    position: absolute;
    top: var(--s4);
    right: var(--s4);
    z-index: 2;
  }
  .nav {
    position: absolute;
    z-index: 2;
    top: 50%;
    translate: 0 -50%;
    width: calc(44px * var(--k));
    min-height: calc(44px * var(--k));
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
    left: 50%;
    translate: -50% 0;
    z-index: 1;
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
  .acts {
    position: absolute;
    left: var(--s4);
    top: var(--s4);
    z-index: 2;
    display: flex;
    gap: var(--s2);
  }
  .act {
    background: rgb(40 40 44 / 0.86);
    color: #f5f5f7;
    box-shadow: none;
    backdrop-filter: blur(10px);
  }
  .act.on {
    background: var(--warning);
    color: #0b0b0c;
  }
  .act .k {
    padding: 0 6px;
    border-radius: 4px;
    background: rgb(255 255 255 / 0.16);
    font: 600 var(--fs-caption) var(--mono);
  }
  .act.on .k {
    background: rgb(0 0 0 / 0.16);
  }
  .res {
    display: inline-flex;
    align-items: center;
    gap: var(--s2);
    font-variant-numeric: tabular-nums;
    color: #d8d8de;
  }
  .res.warn {
    color: var(--warning);
  }
  .res .spinner {
    width: 11px;
    height: 11px;
    border-width: 1.5px;
    border-color: rgb(255 255 255 / 0.35);
    border-top-color: #fff;
  }
  .zoom {
    font-variant-numeric: tabular-nums;
    font-weight: 600;
  }
  .keys {
    color: #b8b8c0;
  }
  @media (max-width: 640px) {
    .stage {
      inset: calc(var(--s6) + var(--s5)) var(--s2) calc(var(--s6) + var(--s5)) var(--s2);
    }
    .keys {
      display: none;
    }
  }
</style>
