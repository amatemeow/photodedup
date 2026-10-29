// The only thing the UI knows about the backend: HTTP to the same origin,
// authenticated with the session token the server put in the URL it opened.
// Shell-agnostic on purpose -- the browser fallback and the Tauri shell
// load this identical code.

const TOKEN_KEY = 'photodedup.token';

/** Picks the token up from `?token=`, remembers it for reloads, and strips
 * it from the address bar so it doesn't linger in history or screenshots. */
function loadToken(): string {
  const url = new URL(window.location.href);
  const fromUrl = url.searchParams.get('token');
  try {
    if (fromUrl) {
      sessionStorage.setItem(TOKEN_KEY, fromUrl);
      url.searchParams.delete('token');
      history.replaceState(null, '', url.pathname + url.search + url.hash);
      return fromUrl;
    }
    return sessionStorage.getItem(TOKEN_KEY) ?? '';
  } catch {
    return fromUrl ?? '';
  }
}

const token = loadToken();

export type Phase = 'scanning' | 'analyzing' | 'analyzing_videos' | 'exact_duplicates' | 'matching' | 'videos' | 'clustering';

export type JobStatus =
  | ({ state: 'running'; phase: Phase } & { done?: number; total?: number })
  | { state: 'done'; scan_id: string }
  | { state: 'cancelled' }
  | { state: 'failed'; message: string };

export interface Resume {
  scan_id: string | null;
  started_at: string | null;
}

export interface FileDto {
  id: string;
  relative_path: string;
  file_name: string;
  media_class: 'Image' | 'Video' | string;
  size_bytes: number;
  is_placeholder: boolean;
}

export interface Cluster {
  id: string;
  tier: string;
  keeper_file_id: string;
  /** From the original, `YYYY-MM-DD HH:MM:SS`; null when no member has one. */
  captured_at: string | null;
  reasons: string[];
  members: FileDto[];
}

export interface BurstRun {
  id: string;
  camera_model: string;
  /** In capture-time order. */
  frames: FileDto[];
  captured_at: string | null;
  sharpness: Record<string, number>;
  suggested_file_id: string | null;
}

export interface ReviewPair {
  id: string;
  tier: string;
  reason: string;
  phash_dist: number;
  captured_at: string | null;
  file_a: FileDto | null;
  file_b: FileDto | null;
}

export interface OrphanVideo {
  file: FileDto;
  captured_at: string | null;
  reason: 'no_match' | 'unknown_origin';
}

export interface Analysis {
  origin: string;
  width: number | null;
  height: number | null;
  camera_model: string | null;
  capture_time: string | null;
  duration_seconds: number | null;
}

export interface DecisionRow {
  relative_path: string;
  keep: boolean;
}

export interface PlanItem {
  relative_path: string;
  size_bytes: number;
  is_sibling: boolean;
}
export interface Blocked {
  relative_path: string;
  reason: string;
}
export interface Plan {
  root_path: string;
  items: PlanItem[];
  blocked: Blocked[];
  total_bytes: number;
}
export interface Skipped {
  relative_path: string;
  reason: string;
}
export interface ApplyResult {
  batch_id: string | null;
  moved: number;
  moved_bytes: number;
  skipped: Skipped[];
  blocked: Blocked[];
}
export interface UndoResult {
  restored: number;
  skipped: Skipped[];
}
export interface Batch {
  id: string;
  created_at: string;
  state: 'running' | 'done' | 'partially_undone' | 'undone';
  quarantine_dir: string;
  root_path: string;
  in_quarantine: number;
  in_quarantine_bytes: number;
  restored: number;
}

export type IntensityId = 'efficient' | 'default' | 'performance';
export interface Knob {
  value: number;
  default: number;
  min: number;
  max: number;
}
export type KnobId = 'sequence_similarity' | 'sequence_gap_seconds' | 'match_distance';
export interface Settings {
  scan_intensity: IntensityId;
  tiers: { id: IntensityId; workers: number }[];
  logical_cores: number;
  advanced: Record<KnobId, Knob>;
  /** The preview cache: its size limit, and what it holds now. */
  cache: { budget_mb: Knob; used_bytes: number };
}
/** Send only what changed. */
export type SettingsPatch = Partial<{ scan_intensity: IntensityId; cache_budget_mb: number } & Record<KnobId, number>>;

/** How many entries each review tab holds (or would hold under other settings). */
export interface Counts {
  exact: number;
  duplicates: number;
  sequences: number;
  sequence_photos: number;
  look_alikes: number;
}
export interface RegroupPreview {
  current: Counts;
  proposed: Counts;
}

export interface ScanInfo {
  id: string;
  root_path: string;
  started_at: string;
  /** What the scan was made with; null for a scan saved before this was recorded. */
  used: { match_distance: number; sequence_similarity: number; sequence_gap_seconds: number; include_videos: boolean } | null;
}

export interface Shell {
  native_folder_picker: boolean;
  tools: { name: string; found: boolean; version: string | null }[];
}

export interface ScanSummary {
  id: string;
  root_path: string;
  started_at: string;
  file_count: number;
  cluster_count: number;
  review_pair_count: number;
  burst_run_count: number;
}

export class ApiError extends Error {
  constructor(
    public status: number,
    message: string,
  ) {
    super(message);
  }
}

async function call<T>(path: string, init: RequestInit = {}): Promise<T> {
  const res = await fetch(path, {
    ...init,
    headers: { ...init.headers, Authorization: `Bearer ${token}`, ...(init.body ? { 'Content-Type': 'application/json' } : {}) },
  });
  if (!res.ok) throw new ApiError(res.status, (await res.text()) || res.statusText);
  const text = await res.text();
  return (text ? JSON.parse(text) : undefined) as T;
}

const THUMB_SIZE = 800;

export const api = {
  hasToken: token !== '',
  resume: (root: string) => call<Resume>(`/api/resume?root=${encodeURIComponent(root)}`),
  startScan: (root: string, opts: { mode?: string; includeVideos?: boolean } = {}) =>
    call<{ job_id: number }>('/api/scans/start', {
      method: 'POST',
      body: JSON.stringify({ root, mode: opts.mode, include_videos: opts.includeVideos }),
    }),
  cancel: (jobId: number) => call<void>(`/api/jobs/${jobId}/cancel`, { method: 'POST' }),
  jobStatus: (jobId: number) => call<JobStatus>(`/api/jobs/${jobId}`),
  clusters: (scanId: string) => call<Cluster[]>(`/api/scans/${scanId}/clusters`),
  burstRuns: (scanId: string) => call<BurstRun[]>(`/api/scans/${scanId}/burst-runs`),
  reviewPairs: (scanId: string) => call<ReviewPair[]>(`/api/scans/${scanId}/review-pairs`),
  orphanVideos: (scanId: string) => call<OrphanVideo[]>(`/api/scans/${scanId}/orphan-videos`),
  analysis: (fileId: string) => call<Analysis>(`/api/files/${fileId}/analysis`),
  decisions: (scanId: string) => call<Record<string, boolean>>(`/api/scans/${scanId}/decisions`),
  putDecisions: (scanId: string, rows: DecisionRow[]) =>
    call<void>(`/api/scans/${scanId}/decisions/bulk`, { method: 'POST', body: JSON.stringify(rows) }),
  /** Back to undecided for every path, in one transaction. */
  clearDecisions: (scanId: string, relativePaths: string[]) =>
    call<void>(`/api/scans/${scanId}/decisions/clear`, { method: 'POST', body: JSON.stringify(relativePaths) }),
  clearDecision: (scanId: string, relativePath: string) =>
    call<void>(`/api/scans/${scanId}/decisions?relative_path=${encodeURIComponent(relativePath)}`, { method: 'DELETE' }),

  scans: () => call<ScanSummary[]>('/api/scans'),
  shell: () => call<Shell>('/api/shell'),
  /** The shell's native folder dialog; null if the user cancels it. */
  pickFolder: () => call<{ path: string | null }>('/api/pick-folder', { method: 'POST' }),
  scanInfo: (scanId: string) => call<ScanInfo>(`/api/scans/${scanId}/info`),
  /** What matching values would change in a scan's results; changes nothing. 409 for a scan that cannot be regrouped. */
  previewRegroup: (scanId: string, values: Record<KnobId, number>) =>
    call<RegroupPreview>(`/api/scans/${scanId}/regroup/preview?${new URLSearchParams(Object.entries(values).map(([k, v]) => [k, String(v)]))}`),
  /** Groups the scan again with the saved matching settings, keeping the user's choices. */
  regroup: (scanId: string) => call<Counts>(`/api/scans/${scanId}/regroup`, { method: 'POST' }),
  settings: () => call<Settings>('/api/settings'),
  /** Start making the grid thumbnails in the background (quietly, within the cache limit). Safe to repeat. */
  prerender: (scanId: string) => call<{ running: boolean; done: number; total: number }>(`/api/scans/${scanId}/prerender`, { method: 'POST' }),
  /** The photos around where the user is looking, most likely first: their previews are made ahead and kept. [] releases. */
  prefetch: (fileIds: string[]) => call<void>('/api/prefetch', { method: 'POST', body: JSON.stringify({ ids: fileIds }) }),
  clearCache: () => call<void>('/api/cache', { method: 'DELETE' }),
  saveSettings: (patch: SettingsPatch) => call<Settings>('/api/settings', { method: 'PUT', body: JSON.stringify(patch) }),

  applyPlan: (scanId: string) => call<Plan>(`/api/scans/${scanId}/apply/plan`),
  apply: (scanId: string) =>
    call<ApplyResult>(`/api/scans/${scanId}/apply`, { method: 'POST', body: JSON.stringify({ confirm: true }) }),
  batches: () => call<Batch[]>('/api/batches'),
  undoBatch: (batchId: string) => call<UndoResult>(`/api/batches/${batchId}/undo`, { method: 'POST' }),

  /** For `<img>`/`<video>` `src`, which can't send an Authorization header.
   * Grid thumbnails ask for `THUMB_SIZE`: tiles are 220+ CSS px wide and cropped to
   * fill (`object-fit: cover`), so a portrait photo needs about 700-800 px on its long
   * edge to stay sharp on a retina screen. One size everywhere, so the grid and the
   * detail view's placeholder share a cache entry. */
  fileUrl: (fileId: string, kind: 'thumb' | 'preview' | 'original') =>
    `/files/${fileId}/${kind}?token=${encodeURIComponent(token)}${kind === 'thumb' ? `&size=${THUMB_SIZE}` : ''}`,

  /** Streams a job's status. EventSource can't send headers, hence the
   * query token. Returns a function that closes the stream. */
  watchJob(jobId: number, onStatus: (s: JobStatus) => void, onLost: () => void): () => void {
    const es = new EventSource(`/api/jobs/${jobId}/events?token=${encodeURIComponent(token)}`);
    es.onmessage = (e) => {
      const status = JSON.parse(e.data) as JobStatus;
      onStatus(status);
      // The server closes right after the terminal event; closing here
      // too stops EventSource from auto-reconnecting to a finished job.
      if (status.state !== 'running') es.close();
    };
    es.onerror = () => {
      es.close();
      onLost();
    };
    return () => es.close();
  },
};
