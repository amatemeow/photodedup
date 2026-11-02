<script module lang="ts">
  export type Name =
    | 'folder'
    | 'sliders'
    | 'dial'
    | 'archive'
    | 'undo'
    | 'check'
    | 'chevron-left'
    | 'chevron-right'
    | 'x'
    | 'help'
    | 'arrow-right'
    | 'photo'
    | 'film'
    | 'sparkles'
    | 'layers'
    | 'burst'
    | 'compare'
    | 'play-off'
    | 'pencil'
    | 'circle'
    | 'tray'
    | 'warning'
    | 'info';
</script>

<script lang="ts">
  /** A small set of line icons, drawn on a 24px grid. Decorative by default
   * (the text next to them carries the meaning); pass `label` when an icon
   * stands alone, such as in an icon-only button. */
  let { name, size = 16, label }: { name: Name; size?: number; label?: string } = $props();

  const paths: Record<Name, string> = {
    folder: 'M3 7a2 2 0 0 1 2-2h4l2 2h8a2 2 0 0 1 2 2v8a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V7Z',
    sliders: 'M4 7h8M18 7h2M4 17h2M12 17h8M15 4.5a2.5 2.5 0 1 0 0 5 2.5 2.5 0 0 0 0-5ZM9 14.5a2.5 2.5 0 1 0 0 5 2.5 2.5 0 0 0 0-5Z',
    // a tuning knob: a dial with its pointer
    dial: 'M12 4a8 8 0 1 0 0 16 8 8 0 0 0 0-16ZM12 12l3.6-3.6M12 6.5v.01M7.6 8.2l.01.01M6.5 12h.01',
    // a box with a lid: put away, not thrown away
    archive: 'M4 5h16v4H4ZM5.5 9v9a1.5 1.5 0 0 0 1.5 1.5h10a1.5 1.5 0 0 0 1.5-1.5V9M10 13h4',
    // a triangle with an exclamation mark: look at this
    warning: 'M12 4 3 20h18L12 4ZM12 10v4.5M12 17.5v.01',
    undo: 'M9 14 4 9l5-5M4 9h10a6 6 0 0 1 0 12h-3',
    check: 'm5 12 5 5 9-10',
    'chevron-left': 'm15 5-7 7 7 7',
    'chevron-right': 'm9 5 7 7-7 7',
    x: 'M6 6l12 12M18 6 6 18',
    help: 'M9.5 9a2.5 2.5 0 1 1 3.5 2.3c-.7.4-1 .9-1 1.7M12 17h.01M12 3a9 9 0 1 0 0 18 9 9 0 0 0 0-18Z',
    'arrow-right': 'M5 12h14m-6-6 6 6-6 6',
    photo: 'M4 6a2 2 0 0 1 2-2h12a2 2 0 0 1 2 2v12a2 2 0 0 1-2 2H6a2 2 0 0 1-2-2V6Zm0 11 4.5-4.5a2 2 0 0 1 2.8 0L16 17m-2-2 1.5-1.5a2 2 0 0 1 2.8 0L20 15M9 9h.01',
    film: 'M4 5h16v14H4V5Zm4 0v14m8-14v14M4 9h4m8 0h4M4 15h4m8 0h4',
    sparkles: 'M12 3v4m0 10v4M3 12h4m10 0h4M6.3 6.3l2.4 2.4m6.6 6.6 2.4 2.4m0-11.4-2.4 2.4m-6.6 6.6-2.4 2.4',
    layers: 'm12 3 9 5-9 5-9-5 9-5Zm-9 9 9 5 9-5m-18 4 9 5 9-5',
    burst: 'M5 7h6v10H5V7Zm8-2h6v10h-6V5Z',
    compare: 'M4 5h7v14H4V5Zm9 0h7v14h-7V5Z',
    'play-off': 'M5 5l14 14M9 5l9 7-3 2',
    pencil: 'm4 20 1-4L16 5a2.1 2.1 0 0 1 3 3L8 19l-4 1Z',
    circle: 'M12 4a8 8 0 1 0 0 16 8 8 0 0 0 0-16Z',
    tray: 'M4 14v4a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2v-4M4 14l2-8h12l2 8M4 14h5l1 2h4l1-2h5',
    info: 'M12 3a9 9 0 1 0 0 18 9 9 0 0 0 0-18Zm0 6h.01M11 12h1v5h1',
  };
</script>

<svg
  width={size}
  height={size}
  viewBox="0 0 24 24"
  fill="none"
  stroke="currentColor"
  stroke-width="2"
  stroke-linecap="round"
  stroke-linejoin="round"
  role={label ? 'img' : undefined}
  aria-label={label}
  aria-hidden={label ? undefined : 'true'}
  style="flex: none"
>
  <path d={paths[name]} />
</svg>
