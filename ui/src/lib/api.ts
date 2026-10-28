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

export type Phase = 'scanning' | 'analyzing' | 'exact_duplicates' | 'matching' | 'videos' | 'clustering';

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
  reasons: string[];
  members: FileDto[];
}

export interface BurstRun {
  id: string;
  camera_model: string;
  /** In capture-time order. */
  frames: FileDto[];
  sharpness: Record<string, number>;
  suggested_file_id: string | null;
}

export interface ReviewPair {
  id: string;
  tier: string;
  reason: string;
  phash_dist: number;
  file_a: FileDto | null;
  file_b: FileDto | null;
}

export interface OrphanVideo {
  file: FileDto;
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

export const api = {
  hasToken: token !== '',
  resume: (root: string) => call<Resume>(`/api/resume?root=${encodeURIComponent(root)}`),
  startScan: (root: string, mode?: string) =>
    call<{ job_id: number }>('/api/scans/start', { method: 'POST', body: JSON.stringify({ root, mode }) }),
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
  clearDecision: (scanId: string, relativePath: string) =>
    call<void>(`/api/scans/${scanId}/decisions?relative_path=${encodeURIComponent(relativePath)}`, { method: 'DELETE' }),

  /** For `<img>`/`<video>` `src`, which can't send an Authorization header. */
  fileUrl: (fileId: string, kind: 'thumb' | 'preview' | 'original') =>
    `/files/${fileId}/${kind}?token=${encodeURIComponent(token)}`,

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
