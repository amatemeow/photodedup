<script lang="ts">
  import { api, ApiError, type IntensityId, type JobStatus, type Phase, type ScanSummary, type Shell } from './lib/api';
  import Review from './Review.svelte';
  import Options from './Options.svelte';
  import Quarantine from './Quarantine.svelte';
  import Icon from './Icon.svelte';
  import ConfirmRescan from './ConfirmRescan.svelte';
  import { cloudLine } from './lib/cloud';
  import { formatBytes } from './lib/review.svelte';

  type View =
    | { kind: 'start' }
    | { kind: 'found'; scanId: string; startedAt: string }
    | { kind: 'scanning'; jobId: number; status: JobStatus; withVideos: boolean }
    | { kind: 'review'; scanId: string }
    | { kind: 'ended'; message: string };

  // The scan, in the order the engine does it.
  const phases: { id: Phase; label: string; videosOnly?: boolean }[] = [
    { id: 'scanning', label: 'Finding files' },
    { id: 'analyzing', label: 'Reading and fingerprinting photos' },
    { id: 'analyzing_videos', label: 'Reading video details', videosOnly: true },
    { id: 'exact_duplicates', label: 'Checking for exact copies' },
    { id: 'matching', label: 'Looking for similar photos' },
    { id: 'videos', label: 'Matching videos against their originals', videosOnly: true },
    { id: 'clustering', label: 'Grouping duplicates' },
  ];

  // Remembered between sessions. Storage can be unavailable (private windows),
  // so every access is guarded and the default (videos included) applies.
  const VIDEOS_KEY = 'photodedup.includeVideos';
  let includeVideos = $state(true);
  try {
    includeVideos = localStorage.getItem(VIDEOS_KEY) !== 'false';
  } catch {}
  $effect(() => {
    try {
      localStorage.setItem(VIDEOS_KEY, String(includeVideos));
    } catch {}
  });

  // Shown on the start screen so it is clear how the next scan will run.
  let intensity = $state<IntensityId | null>(null);
  const intensityName: Record<IntensityId, string> = { efficient: 'Efficient', default: 'Default', performance: 'Performance' };
  async function loadIntensity() {
    try {
      intensity = (await api.settings()).scan_intensity;
    } catch {
      intensity = null; // the hint is a convenience; the scan works without it
    }
  }
  loadIntensity();

  // What the surrounding app can do: a native folder dialog only exists in the
  // desktop shell, and ffmpeg/ffprobe are needed for HEIC photos and videos.
  let shell = $state<Shell | null>(null);
  api.shell().then((s) => (shell = s)).catch(() => {}); // a convenience; the app works without it
  const missingTools = $derived(shell?.tools.filter((t) => !t.found).map((t) => t.name) ?? []);

  // The scan already in the database, offered as "continue where you left off".
  let last = $state<ScanSummary | null>(null);
  let lastLoaded = $state(false);
  async function loadLast() {
    try {
      last = (await api.scans())[0] ?? null;
    } catch {
      last = null;
    } finally {
      lastLoaded = true;
    }
  }
  loadLast();

  let root = $state('');
  let view = $state<View>({ kind: 'start' });
  let busy = $state(false);
  // Settings opens over whatever screen is showing without replacing it, so a
  // scan keeps running behind it and closing it returns to exactly where you were.
  let settingsOpen = $state(false);
  let quarantineOpen = $state(false);
  let error = $state('');
  let stopWatching: (() => void) | undefined;

  async function browse() {
    error = '';
    try {
      const { path } = await api.pickFolder();
      if (path) root = path;
    } catch (e) {
      error = describe(e);
    }
  }

  async function check() {
    error = '';
    busy = true;
    try {
      // Opening a folder we already scanned resumes it (user's design
      // choice) -- rescanning is the explicit alternative, not the default.
      const r = await api.resume(root.trim());
      if (r.scan_id !== null) view = { kind: 'found', scanId: r.scan_id, startedAt: r.started_at ?? '' };
      else await scan();
    } catch (e) {
      error = describe(e);
    } finally {
      busy = false;
    }
  }

  const scan = () => startScan(root.trim(), includeVideos);

  // Repeats the last scan on its own folder, with the include-videos choice it
  // was made with, so there is no need to pick the folder again.
  let rescanning = $state(false);
  // A re-scan replaces the current results, so it is always asked about first.
  let confirm = $state<{ folder: string; run: () => void } | null>(null);
  async function rescanLast() {
    if (!last) return;
    rescanning = true;
    try {
      const used = (await api.scanInfo(last.id).catch(() => null))?.used;
      await startScan(last.root_path, used?.include_videos ?? includeVideos);
    } finally {
      rescanning = false;
    }
  }

  // Also used by the review's "Scan again", which repeats a scan with the
  // same folder and the same include-videos choice it was made with.
  async function startScan(folder: string, withVideos: boolean) {
    error = '';
    try {
      const { job_id } = await api.startScan(folder, { includeVideos: withVideos });
      view = { kind: 'scanning', jobId: job_id, status: { state: 'running', phase: 'scanning' }, withVideos };
      stopWatching = api.watchJob(job_id, onStatus, onLost);
    } catch (e) {
      error = describe(e);
      view = { kind: 'start' };
    }
  }

  async function onStatus(status: JobStatus) {
    if (view.kind !== 'scanning') return;
    if (status.state === 'running') view = { ...view, status };
    else if (status.state === 'done') open(status.scan_id);
    else if (status.state === 'cancelled') {
      view = { kind: 'start' };
      error = 'Scan cancelled.';
    } else view = { kind: 'ended', message: status.message };
  }

  // The stream dropped. The job may still be running or may have finished
  // while we were disconnected, so ask the snapshot endpoint rather than
  // guessing.
  async function onLost() {
    if (view.kind !== 'scanning') return;
    try {
      const s = await api.jobStatus(view.jobId);
      if (s.state === 'running') stopWatching = api.watchJob(view.jobId, onStatus, onLost);
      else await onStatus(s);
    } catch {
      view = { kind: 'ended', message: 'Lost contact with the scanner.' };
    }
  }

  function open(scanId: string) {
    settingsOpen = false;
    view = { kind: 'review', scanId };
  }

  function backToStart() {
    view = { kind: 'start' };
    loadLast();
  }

  async function cancel() {
    if (view.kind !== 'scanning') return;
    try {
      await api.cancel(view.jobId);
    } catch (e) {
      error = describe(e);
    }
  }

  function describe(e: unknown): string {
    if (e instanceof ApiError) return e.status === 401 ? 'Not authorised: open the link the app printed, token included.' : e.message;
    return e instanceof Error ? e.message : String(e);
  }

  // A determinate bar wherever the engine reports a count (photos, then
  // videos); an indeterminate one for the short phases without one.
  const hasCount = (s: JobStatus) =>
    s.state === 'running' && (s.phase === 'analyzing' || s.phase === 'analyzing_videos' || s.phase === 'videos') && !!s.total;
  const percent = (s: JobStatus) => (hasCount(s) && s.state === 'running' ? Math.round(((s.done ?? 0) / s.total!) * 100) : null);

  // Photos and videos are separate phases with separate counts, so each
  // says what it counts.
  const detail = (s: JobStatus) =>
    !hasCount(s) || s.state !== 'running' ? '' : `${s.done} of ${s.total} ${s.phase === 'analyzing' ? 'photos' : 'videos'}`;

  const folderName = (path: string) => path.split(/[\\/]/).filter(Boolean).pop() ?? path;
  function ago(iso: string): string {
    const secs = (new Date(iso).getTime() - Date.now()) / 1000;
    const rtf = new Intl.RelativeTimeFormat(undefined, { numeric: 'auto' });
    for (const [unit, size] of [['day', 86400], ['hour', 3600], ['minute', 60]] as const) {
      if (Math.abs(secs) >= size) return rtf.format(Math.round(secs / size), unit);
    }
    return 'just now';
  }
  const groupCount = (s: ScanSummary) => s.cluster_count + s.burst_run_count;

  const shownPhases = (withVideos: boolean) => phases.filter((p) => withVideos || !p.videosOnly);
  const phaseIndex = (withVideos: boolean, s: JobStatus) =>
    s.state === 'running' ? shownPhases(withVideos).findIndex((p) => p.id === s.phase) : -1;
</script>

{#if confirm}
  <ConfirmRescan
    folder={confirm.folder}
    onCancel={() => (confirm = null)}
    onConfirm={() => {
      const run = confirm!.run;
      confirm = null;
      run();
    }}
  />
{/if}

{#if settingsOpen && view.kind !== 'review'}
  <Options
    scope={view.kind === 'scanning' ? 'scanning' : 'start'}
    onBack={() => {
      settingsOpen = false;
      loadIntensity();
    }}
  />
{:else if view.kind === 'review'}
  <Review scanId={view.scanId} onExit={backToStart} onRescan={startScan} />
{:else}
  <main>
    <header class="top">
      <button class="btn plain" onclick={() => (quarantineOpen = true)}><Icon name="archive" /> Quarantine</button>
      <button class="btn plain" onclick={() => (settingsOpen = true)}><Icon name="sliders" /> Settings</button>
    </header>

    <div class="column">
      <div class="brand">
        <svg class="mark" viewBox="0 0 64 64" aria-hidden="true">
          <defs>
            <linearGradient id="g" x1="0" y1="0" x2="0" y2="1"><stop offset="0" stop-color="#2d6cf0" /><stop offset="1" stop-color="#1a3fa6" /></linearGradient>
          </defs>
          <rect x="2" y="2" width="60" height="60" rx="14" fill="url(#g)" />
          <rect x="15" y="12" width="28" height="28" rx="4" fill="#fff" fill-opacity=".45" />
          <rect x="21" y="20" width="28" height="28" rx="4" fill="#fff" />
          <circle cx="40" cy="28" r="3" fill="#f6b93b" />
          <path d="M21 48v-6c4-5 8-5 11-2 3-5 9-6 17 0v8Z" fill="#2f8f57" />
        </svg>
        <h1 class="title display">photodedup</h1>
        <p class="secondary lead">Find duplicate photos and videos. Nothing is ever deleted.</p>
      </div>

      {#if !api.hasToken}
        <div class="notice warning" role="alert"><Icon name="info" size={18} /><p>No session token. Open the URL the app printed when it started.</p></div>
      {/if}

      {#if view.kind === 'start'}
        {#if missingTools.length}
          <div class="notice warning" role="alert">
            <Icon name="info" size={18} />
            <p>
              <strong>{missingTools.join(' and ')} {missingTools.length > 1 ? 'were' : 'was'} not found.</strong> Without
              {missingTools.length > 1 ? 'them' : 'it'}, HEIC photos and videos can't be read (ordinary JPEGs still work). Install ffmpeg and
              restart the app.
            </p>
          </div>
        {/if}

        {#if last}
          <section class="card resume" aria-label="Continue where you left off">
            <div class="resume-text">
              <p class="caption">Continue where you left off</p>
              <p class="lead name"><Icon name="folder" size={18} /> {folderName(last.root_path)}</p>
              <p class="hint">{groupCount(last)} groups to review · scanned {ago(last.started_at)}</p>
            </div>
            <div class="resume-actions">
              <button
                class="btn"
                disabled={rescanning}
                onclick={() => (confirm = { folder: folderName(last!.root_path), run: rescanLast })}
                title="Scan this folder again. The new scan replaces this one; your choices on files that haven't changed are kept."
              >
                {rescanning ? 'Starting…' : 'Scan again'}
              </button>
              <button class="btn primary" onclick={() => open(last!.id)}>Continue <Icon name="arrow-right" /></button>
            </div>
          </section>
        {/if}

        <form class="card" onsubmit={(e) => { e.preventDefault(); check(); }}>
          <h2 class="lead">{last ? 'Scan another folder' : 'Scan a folder'}</h2>

          <div class="row">
            <input
              id="root"
              class="field grow"
              bind:value={root}
              placeholder={shell?.native_folder_picker ? 'Choose a folder, or paste its path' : '/Users/me/Pictures/Trip'}
              aria-label="Folder to scan"
              autocomplete="off"
              spellcheck="false"
            />
            {#if shell?.native_folder_picker}
              <button type="button" class="btn" onclick={browse}><Icon name="folder" /> Choose…</button>
            {/if}
          </div>

          <label class="switch setting">
            <input type="checkbox" bind:checked={includeVideos} />
            <span>
              <strong>Include videos</strong>
              <span class="hint block">Turn off to skip them entirely: faster, and they are neither compared nor listed.</span>
            </span>
          </label>

          <button class="btn primary large" disabled={busy || !root.trim()}>{busy ? 'Checking…' : 'Scan'}</button>
          {#if intensity}
            <p class="hint center">Scan speed: {intensityName[intensity]} · change it in Settings</p>
          {/if}
        </form>

        {#if !lastLoaded}<div class="skeleton" style="height:0"></div>{/if}
      {:else if view.kind === 'found'}
        <section class="card">
          <h2 class="lead">You've scanned this folder before</h2>
          <p class="secondary">
            {folderName(root)} was scanned{view.startedAt ? ` ${ago(view.startedAt)}` : ''}. Resume to carry on where you stopped.
            Scanning again replaces that scan; the choices you made on files that haven't changed are kept.
          </p>
          <div class="row">
            <button class="btn primary" onclick={() => open((view as { scanId: string }).scanId)}>Resume</button>
            <button class="btn" onclick={() => (confirm = { folder: folderName(root), run: scan })}>Scan again</button>
            <button class="btn plain" onclick={() => (view = { kind: 'start' })}>Back</button>
          </div>
        </section>
      {:else if view.kind === 'scanning'}
        {@const s = view.status}
        {@const current = phaseIndex(view.withVideos, s)}
        {@const pct = percent(s)}
        <section class="card" aria-live="polite">
          <div class="row between">
            <h2 class="lead">Scanning…</h2>
            <button class="btn plain" onclick={cancel}>Cancel</button>
          </div>
          <progress max="100" value={pct ?? undefined} aria-label="Scan progress"></progress>
          <p class="hint">{detail(s) || ' '}</p>
          {#if s.state === 'running' && cloudLine(s.cloud, formatBytes)}<p class="hint cloud" role="status">{cloudLine(s.cloud, formatBytes)}</p>{/if}
          <ol class="steps">
            {#each shownPhases(view.withVideos) as p, i (p.id)}
              <li class:done={i < current} class:now={i === current}>
                {#if i < current}<Icon name="check" size={16} />{:else if i === current}<span class="spinner"></span>{:else}<span class="dot"></span>{/if}
                <span>{p.label}</span>
              </li>
            {/each}
          </ol>
        </section>
      {:else}
        <section class="card">
          <div class="notice danger" role="alert"><Icon name="info" size={18} /><p><strong>The scan didn't finish.</strong> {view.message}</p></div>
          <button class="btn" onclick={backToStart}>Back</button>
        </section>
      {/if}

      {#if error}
        <div class="notice danger" role="alert"><Icon name="info" size={18} /><p>{error}</p></div>
      {/if}
    </div>
  </main>
{/if}

{#if quarantineOpen && view.kind !== 'review'}
  <Quarantine onClose={() => (quarantineOpen = false)} onChanged={() => {}} />
{/if}

<style>
  main {
    min-height: 100%;
    display: flex;
    flex-direction: column;
  }
  .top {
    display: flex;
    justify-content: flex-end;
    gap: var(--s2);
    padding: var(--s3) var(--s4);
    min-height: calc(54px * var(--k));
  }
  .column {
    width: min(calc(520px * var(--k)), 100%);
    margin: 0 auto;
    padding: var(--s2) var(--s4) var(--s7);
    display: grid;
    gap: var(--s4);
    align-content: start;
  }
  .brand {
    display: grid;
    justify-items: center;
    gap: var(--s2);
    padding: var(--s4) 0 var(--s2);
    text-align: center;
  }
  .mark {
    width: calc(72px * var(--k));
    height: calc(72px * var(--k));
    filter: drop-shadow(0 6px 14px rgb(0 0 0 / 0.22));
  }
  .display {
    font-size: var(--fs-display);
  }
  .card {
    display: grid;
    gap: var(--s4);
    padding: var(--s5);
    border-radius: var(--radius-lg);
    background: var(--surface);
    box-shadow: var(--shadow-sm);
  }
  .resume {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    justify-content: space-between;
  }
  .resume-actions {
    display: flex;
    gap: var(--s2);
    flex-wrap: wrap;
    justify-content: flex-end;
  }
  .resume-text {
    display: grid;
    gap: 2px;
    min-width: 0;
    flex: 1 1 calc(200px * var(--k));
  }
  .name {
    display: flex;
    align-items: center;
    gap: var(--s2);
    font-weight: 600;
    min-width: 0;
  }
  .row {
    display: flex;
    gap: var(--s2);
    align-items: center;
    flex-wrap: wrap;
  }
  .row.between {
    justify-content: space-between;
  }
  .grow {
    flex: 1;
    min-width: 0;
  }
  .setting {
    align-items: flex-start;
  }
  .setting strong {
    font-weight: 600;
  }
  .block {
    display: block;
  }
  .center {
    text-align: center;
  }
  .steps {
    list-style: none;
    margin: 0;
    padding: 0;
    display: grid;
    gap: var(--s2);
  }
  .steps li {
    display: flex;
    align-items: center;
    gap: var(--s3);
    color: var(--label-3);
  }
  .steps li.done {
    color: var(--success);
  }
  .steps li.done span:last-child {
    color: var(--label-2);
  }
  .steps li.now {
    color: var(--label);
    font-weight: 600;
  }
  .dot {
    width: 16px;
    height: 16px;
    flex: none;
    display: grid;
    place-items: center;
  }
  .dot::after {
    content: '';
    width: 6px;
    height: 6px;
    border-radius: 50%;
    background: var(--separator);
  }
  .steps :global(svg) {
    flex: none;
  }
</style>
