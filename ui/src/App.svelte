<script lang="ts">
  import { api, ApiError, type JobStatus, type Phase } from './lib/api';

  type View =
    | { kind: 'start' }
    | { kind: 'found'; scanId: number; startedAt: string }
    | { kind: 'scanning'; jobId: number; status: JobStatus }
    | { kind: 'ready'; scanId: number; clusters: number }
    | { kind: 'ended'; message: string };

  const phaseLabel: Record<Phase, string> = {
    scanning: 'Finding files',
    analyzing: 'Reading and fingerprinting photos',
    exact_duplicates: 'Checking for exact copies',
    matching: 'Looking for similar photos',
    videos: 'Matching videos',
    clustering: 'Grouping duplicates',
  };

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
      const { job_id } = await api.startScan(root.trim());
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
    else if (status.state === 'done') await open(status.scan_id);
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

  async function open(scanId: number) {
    try {
      view = { kind: 'ready', scanId, clusters: (await api.clusters(scanId)).length };
    } catch (e) {
      view = { kind: 'ended', message: describe(e) };
    }
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

  const percent = (s: JobStatus) =>
    s.state === 'running' && s.phase === 'analyzing' && s.total ? Math.round(((s.done ?? 0) / s.total) * 100) : null;
</script>

<main>
  <h1>photodedup</h1>

  {#if !api.hasToken}
    <p class="note">No session token. Open the URL the app printed when it started.</p>
  {/if}

  {#if view.kind === 'start'}
    <form onsubmit={(e) => { e.preventDefault(); check(); }}>
      <label for="root">Folder to scan</label>
      <input id="root" bind:value={root} placeholder="/Users/me/Pictures/Trip" autocomplete="off" spellcheck="false" />
      <button disabled={busy || !root.trim()}>{busy ? 'Checking…' : 'Scan'}</button>
    </form>
    <p class="hint">Type or paste the folder path. A native folder picker arrives with the desktop shell.</p>
  {:else if view.kind === 'found'}
    <p>This folder was scanned before{view.startedAt ? ` (${new Date(view.startedAt).toLocaleString()})` : ''}.</p>
    <div class="row">
      <button onclick={() => open((view as { scanId: number }).scanId)}>Resume</button>
      <button class="secondary" onclick={scan}>Scan again</button>
      <button class="secondary" onclick={() => (view = { kind: 'start' })}>Back</button>
    </div>
  {:else if view.kind === 'scanning'}
    {@const s = view.status}
    {@const pct = percent(s)}
    <p>{s.state === 'running' ? phaseLabel[s.phase] : ''}…</p>
    <progress max="100" value={pct ?? undefined}></progress>
    <p class="hint">
      {#if s.state === 'running' && s.phase === 'analyzing' && s.total}
        {s.done} of {s.total} photos and videos
      {:else}&nbsp;{/if}
    </p>
    <button class="secondary" onclick={cancel}>Cancel</button>
  {:else if view.kind === 'ready'}
    <p>Scan #{view.scanId} is ready: {view.clusters} groups of duplicates found.</p>
    <p class="hint">The review screen is the next piece of the UI.</p>
    <button class="secondary" onclick={() => (view = { kind: 'start' })}>Scan another folder</button>
  {:else}
    <p class="error">Scan failed: {view.message}</p>
    <button class="secondary" onclick={() => (view = { kind: 'start' })}>Back</button>
  {/if}

  {#if error}<p class="error" role="alert">{error}</p>{/if}
</main>

<style>
  :global(:root) {
    --bg: #fafafa;
    --fg: #1d1d1f;
    --muted: #6e6e73;
    --line: #d2d2d7;
    --accent: #0a64d8;
    --accent-fg: #fff;
    --danger: #c4291c;
    color-scheme: light;
  }
  @media (prefers-color-scheme: dark) {
    :global(:root) {
      --bg: #1c1c1e;
      --fg: #f5f5f7;
      --muted: #98989d;
      --line: #3a3a3c;
      --accent: #4c9aff;
      --accent-fg: #0b0b0c;
      --danger: #ff6b5e;
      color-scheme: dark;
    }
  }
  :global(body) {
    margin: 0;
    background: var(--bg);
    color: var(--fg);
    font: 15px/1.5 system-ui, -apple-system, 'Segoe UI', sans-serif;
  }
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
  input {
    font: inherit;
    padding: 0.55rem 0.7rem;
    border: 1px solid var(--line);
    border-radius: 8px;
    background: transparent;
    color: inherit;
  }
  button {
    font: inherit;
    padding: 0.5rem 1rem;
    border: 0;
    border-radius: 8px;
    background: var(--accent);
    color: var(--accent-fg);
    cursor: pointer;
  }
  button:disabled {
    opacity: 0.5;
    cursor: default;
  }
  button.secondary {
    background: transparent;
    color: inherit;
    border: 1px solid var(--line);
  }
  .row {
    display: flex;
    gap: 0.5rem;
    flex-wrap: wrap;
  }
  progress {
    width: 100%;
    accent-color: var(--accent);
  }
  .hint,
  .note {
    color: var(--muted);
    font-size: 0.9em;
  }
  .error {
    color: var(--danger);
  }
</style>
