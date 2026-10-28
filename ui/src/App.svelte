<script lang="ts">
  import { api, ApiError, type JobStatus, type Phase } from './lib/api';
  import Review from './Review.svelte';

  type View =
    | { kind: 'start' }
    | { kind: 'found'; scanId: string; startedAt: string }
    | { kind: 'scanning'; jobId: number; status: JobStatus }
    | { kind: 'review'; scanId: string }
    | { kind: 'ended'; message: string };

  const phaseLabel: Record<Phase, string> = {
    scanning: 'Finding files',
    analyzing: 'Reading and fingerprinting photos',
    analyzing_videos: 'Reading video details',
    exact_duplicates: 'Checking for exact copies',
    matching: 'Looking for similar photos',
    videos: 'Matching videos against their originals',
    clustering: 'Grouping duplicates',
  };

  // Remembered between sessions; both flags move to the Options tab once it
  // exists. Storage can be unavailable (private windows), so every access is
  // guarded and the default (videos included) applies.
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

  let root = $state('');
  let view = $state<View>({ kind: 'start' });
  let busy = $state(false);
  let error = $state('');
  let stopWatching: (() => void) | undefined;

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

  async function scan() {
    error = '';
    try {
      const { job_id } = await api.startScan(root.trim(), { includeVideos });
      view = { kind: 'scanning', jobId: job_id, status: { state: 'running', phase: 'scanning' } };
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
    view = { kind: 'review', scanId };
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
</script>

{#if view.kind === 'review'}
  <Review scanId={view.scanId} onExit={() => (view = { kind: 'start' })} />
{:else}
<main>
  <h1>photodedup</h1>

  {#if !api.hasToken}
    <p class="note">No session token. Open the URL the app printed when it started.</p>
  {/if}

  {#if view.kind === 'start'}
    <form onsubmit={(e) => { e.preventDefault(); check(); }}>
      <label for="root">Folder to scan</label>
      <input id="root" bind:value={root} placeholder="/Users/me/Pictures/Trip" autocomplete="off" spellcheck="false" />
      <label class="check">
        <input type="checkbox" bind:checked={includeVideos} />
        Include videos
      </label>
      <p class="hint tight">Turn off to leave videos out entirely: faster, and they are neither compared nor listed.</p>
      <button disabled={busy || !root.trim()}>{busy ? 'Checking…' : 'Scan'}</button>
    </form>
    <p class="hint">Type or paste the folder path. A native folder picker arrives with the desktop shell.</p>
  {:else if view.kind === 'found'}
    <p>This folder was scanned before{view.startedAt ? ` (${new Date(view.startedAt).toLocaleString()})` : ''}.</p>
    <p class="hint">Scanning again replaces that scan and the keep/quarantine choices you made on it.</p>
    <div class="row">
      <button onclick={() => open((view as { scanId: string }).scanId)}>Resume</button>
      <button class="secondary" onclick={scan}>Scan again</button>
      <button class="secondary" onclick={() => (view = { kind: 'start' })}>Back</button>
    </div>
  {:else if view.kind === 'scanning'}
    {@const s = view.status}
    {@const pct = percent(s)}
    <p>{s.state === 'running' ? phaseLabel[s.phase] : ''}…</p>
    <progress max="100" value={pct ?? undefined}></progress>
    <p class="hint">{detail(s) || '\u00a0'}</p>
    <button class="secondary" onclick={cancel}>Cancel</button>
  {:else}
    <p class="error">Scan failed: {view.message}</p>
    <button class="secondary" onclick={() => (view = { kind: 'start' })}>Back</button>
  {/if}

  {#if error}<p class="error" role="alert">{error}</p>{/if}
</main>
{/if}

<style>
  main {
    max-width: 36rem;
    margin: 0 auto;
    padding: 4rem 1rem;
  }
  h1 {
    font-size: 1.4rem;
    margin: 0 0 2rem;
  }
  form {
    display: grid;
    gap: 0.5rem;
  }
  .check {
    display: flex;
    align-items: center;
    gap: 0.5rem;
  }
  .check input {
    padding: 0;
  }
  .tight {
    margin: -0.25rem 0 0.25rem;
  }
  .row {
    display: flex;
    gap: 0.5rem;
    flex-wrap: wrap;
  }
</style>
