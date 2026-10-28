import { api, type Analysis, type Cluster, type FileDto } from './api';

/** `partial` = some but not all members decided (e.g. an interrupted
 * write); `accepted` = decided exactly as the engine suggested; `changed` =
 * decided differently. */
export type Status = 'undecided' | 'partial' | 'accepted' | 'changed';
export type Filter = 'all' | 'undecided' | 'changed';

interface UndoEntry {
  prev: { path: string; keep: boolean | undefined }[];
}

/** All review state for one scan. Decisions are per *file* (keep or
 * quarantine), so any number of a cluster's members can be kept. The
 * engine's pick is only a suggestion until the user acts on it. */
export class Review {
  clusters = $state<Cluster[]>([]);
  decisions = $state<Record<string, boolean>>({});
  filter = $state<Filter>('all');
  loaded = $state(false);
  error = $state('');
  canUndo = $state(false);

  private undoStack: UndoEntry[] = [];
  private analyses = new Map<string, Promise<Analysis | null>>();

  constructor(readonly scanId: string) {}

  async load() {
    try {
      const [clusters, decisions] = await Promise.all([api.clusters(this.scanId), api.decisions(this.scanId)]);
      this.clusters = clusters;
      this.decisions = decisions;
      this.loaded = true;
    } catch (e) {
      this.error = e instanceof Error ? e.message : String(e);
    }
  }

  suggested = (c: Cluster, f: FileDto) => f.id === c.keeper_file_id;
  effective = (c: Cluster, f: FileDto) => this.decisions[f.relative_path] ?? this.suggested(c, f);
  isDecided = (f: FileDto) => f.relative_path in this.decisions;

  status(c: Cluster): Status {
    const decided = c.members.filter((m) => this.isDecided(m));
    if (decided.length === 0) return 'undecided';
    if (decided.length < c.members.length) return 'partial';
    return c.members.every((m) => this.decisions[m.relative_path] === this.suggested(c, m)) ? 'accepted' : 'changed';
  }

  statuses = $derived(new Map(this.clusters.map((c) => [c.id, this.status(c)] as const)));

  /** Clusters shown under the current filter. `partial` counts as
   * undecided for filtering: it still needs the user's attention. */
  visible = $derived(
    this.clusters.filter((c) => {
      const s = this.statuses.get(c.id);
      if (this.filter === 'undecided') return s === 'undecided' || s === 'partial';
      if (this.filter === 'changed') return s === 'changed';
      return true;
    }),
  );

  counts = $derived.by(() => {
    let undecided = 0;
    let changed = 0;
    for (const s of this.statuses.values()) {
      if (s === 'undecided' || s === 'partial') undecided++;
      if (s === 'changed') changed++;
    }
    return { all: this.clusters.length, undecided, changed, reviewed: this.clusters.length - undecided };
  });

  /** Flips one member between keep and quarantine. Refuses to quarantine
   * the last kept member: a cluster always keeps at least one copy. */
  async toggle(c: Cluster, f: FileDto) {
    const keeps = c.members.map((m) => (m.id === f.id ? !this.effective(c, m) : this.effective(c, m)));
    if (!keeps.some(Boolean)) {
      this.error = 'Keep at least one copy of each photo.';
      return;
    }
    await this.commit(c.members.map((m, i) => ({ path: m.relative_path, keep: keeps[i] })));
  }

  /** Records the engine's suggestion for these clusters as decisions. */
  async accept(clusters: Cluster[]) {
    await this.commit(clusters.flatMap((c) => c.members.map((m) => ({ path: m.relative_path, keep: this.suggested(c, m) }))));
  }

  async acceptAllUndecided() {
    await this.accept(this.clusters.filter((c) => this.statuses.get(c.id) === 'undecided'));
  }

  async undo() {
    const entry = this.undoStack.pop();
    this.canUndo = this.undoStack.length > 0;
    if (!entry) return;
    const restore = entry.prev.filter((p) => p.keep !== undefined);
    const clear = entry.prev.filter((p) => p.keep === undefined);
    try {
      if (restore.length) await api.putDecisions(this.scanId, restore.map((p) => ({ relative_path: p.path, keep: p.keep! })));
      for (const p of clear) await api.clearDecision(this.scanId, p.path);
    } catch (e) {
      this.error = `Undo failed: ${e instanceof Error ? e.message : e}`;
      return;
    }
    for (const p of entry.prev) {
      if (p.keep === undefined) delete this.decisions[p.path];
      else this.decisions[p.path] = p.keep;
    }
  }

  /** Write-through: the screen updates at once, and rolls back with an
   * error if the server refuses. */
  private async commit(rows: { path: string; keep: boolean }[]) {
    this.error = '';
    const prev = rows.map((r) => ({ path: r.path, keep: this.decisions[r.path] }));
    for (const r of rows) this.decisions[r.path] = r.keep;
    try {
      await api.putDecisions(this.scanId, rows.map((r) => ({ relative_path: r.path, keep: r.keep })));
      this.undoStack.push({ prev });
      this.canUndo = true;
    } catch (e) {
      for (const p of prev) {
        if (p.keep === undefined) delete this.decisions[p.path];
        else this.decisions[p.path] = p.keep;
      }
      this.error = `Couldn't save: ${e instanceof Error ? e.message : e}`;
    }
  }

  analysis(fileId: string): Promise<Analysis | null> {
    let p = this.analyses.get(fileId);
    if (!p) {
      p = api.analysis(fileId).catch(() => null);
      this.analyses.set(fileId, p);
    }
    return p;
  }
}

export const tierLabel: Record<string, string> = {
  T0: 'Identical files',
  T1: 'Same photo, different copy',
  T1b: 'Copies of the same photo',
};

export function formatBytes(n: number): string {
  if (n < 1024) return `${n} B`;
  const units = ['KB', 'MB', 'GB', 'TB'];
  let v = n / 1024;
  let i = 0;
  while (v >= 1024 && i < units.length - 1) {
    v /= 1024;
    i++;
  }
  return `${v.toFixed(v < 10 ? 1 : 0)} ${units[i]}`;
}
