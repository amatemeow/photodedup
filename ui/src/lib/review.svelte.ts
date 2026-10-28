import { api, type Analysis, type FileDto } from './api';

/** `partial` = some but not all members decided (e.g. an interrupted
 * write); `accepted` = decided exactly as suggested; `changed` = decided
 * differently. */
export type Status = 'undecided' | 'partial' | 'accepted' | 'changed';
export type Filter = 'all' | 'undecided' | 'changed';

/** The categories the review is split into. Every one is decided the same
 * way (keep / quarantine per file); they differ in what the engine
 * suggests: duplicates and sequences suggest dropping the extras, while
 * look-alikes and orphan videos suggest keeping everything. */
export type Tab = 'exact' | 'true' | 'burst' | 'similar' | 'orphans';
export const tabs: Tab[] = ['exact', 'true', 'burst', 'similar', 'orphans'];
export type Sort = 'oldest' | 'newest';

/** One thing the user decides on: a duplicate cluster, a sequence, a pair
 * of look-alikes or a single orphan video. `suggested` holds the file ids
 * the engine would keep; anything else in the group would be quarantined. */
export interface Group {
  id: string;
  tab: Tab;
  label: string;
  /** When it was taken, from the original (`YYYY-MM-DD HH:MM:SS`), or null. */
  capturedAt: string | null;
  reasons: string[];
  members: FileDto[];
  suggested: Set<string>;
  sharpness: Record<string, number>;
}

export interface Counts {
  all: number;
  undecided: number;
  changed: number;
  reviewed: number;
}

interface UndoEntry {
  prev: { path: string; keep: boolean | undefined }[];
}

/** All review state for one scan. Decisions are per *file*, so any number
 * of a group's members can be kept, and a file that sits in two groups (a
 * burst frame that also has a messenger copy) has one decision, shown in
 * both. The engine's pick is only a suggestion until the user acts on it.
 *
 * Safety rule everywhere a decision is written: no group may be left with
 * nothing kept. Files marked keep are never quarantined by a bulk action. */
export class Review {
  groups = $state.raw<Group[]>([]);
  sort = $state<Sort>(loadSort());
  decisions = $state<Record<string, boolean>>({});
  tab = $state<Tab>('true');
  filter = $state<Filter>('all');
  loaded = $state(false);
  error = $state('');
  canUndo = $state(false);

  private undoStack: UndoEntry[] = [];
  private analyses = new Map<string, Promise<Analysis | null>>();

  constructor(readonly scanId: string) {}

  async load() {
    try {
      const [clusters, bursts, pairs, orphans, decisions] = await Promise.all([
        api.clusters(this.scanId),
        api.burstRuns(this.scanId),
        api.reviewPairs(this.scanId),
        api.orphanVideos(this.scanId),
        api.decisions(this.scanId),
      ]);
      // A burst frame that is some duplicate cluster's suggested keeper
      // must not be thinned away by the burst's own suggestion.
      const clusterKeepers = new Set(clusters.map((c) => c.keeper_file_id));
      this.groups = [
        ...clusters.map<Group>((c) => ({
          id: c.id,
          tab: c.tier === 'T0' ? 'exact' : 'true',
          label: tierLabel[c.tier] ?? c.tier,
          capturedAt: c.captured_at,
          reasons: c.reasons,
          members: c.members,
          suggested: new Set([c.keeper_file_id]),
          sharpness: {},
        })),
        ...bursts.map<Group>((b) => ({
          id: b.id,
          tab: 'burst',
          label: b.camera_model,
          capturedAt: b.captured_at,
          reasons: [],
          members: b.frames,
          suggested: new Set([...(b.suggested_file_id ? [b.suggested_file_id] : []), ...b.frames.filter((f) => clusterKeepers.has(f.id)).map((f) => f.id)]),
          sharpness: b.sharpness,
        })),
      ];
      // Burst-tier pairs are the neighbours inside a burst run, which the
      // Burst tab already shows; this tab is for look-alikes that are NOT
      // part of a burst.
      this.groups = [
        ...this.groups,
        // Burst-tier pairs are the neighbours inside a sequence, which the
        // Quick sequences tab already shows; this tab is for look-alikes
        // that are NOT part of one. Nothing here is suggested for removal.
        ...pairs
          .filter((p) => p.tier !== 'T2Burst')
          .map<Group>((p) => {
            const members = [p.file_a, p.file_b].filter((f): f is FileDto => f !== null);
            return {
              id: p.id,
              tab: 'similar',
              label: p.reason.replaceAll('_', ' '),
              capturedAt: p.captured_at,
              reasons: [],
              members,
              suggested: new Set(members.map((m) => m.id)),
              sharpness: {},
            };
          }),
        // Videos with no counterpart: one per group, kept unless the user
        // says otherwise.
        ...orphans.map<Group>((o) => ({
          id: o.file.id,
          tab: 'orphans',
          label: orphanReason[o.reason] ?? o.reason,
          capturedAt: o.captured_at,
          reasons: [],
          members: [o.file],
          suggested: new Set([o.file.id]),
          sharpness: {},
        })),
      ];
      this.decisions = decisions;
      // Open on the first category that has anything to review.
      this.tab = tabs.find((t) => this.tabCount(t) > 0) ?? 'true';
      this.loaded = true;
    } catch (e) {
      this.error = e instanceof Error ? e.message : String(e);
    }
  }

  suggested = (g: Group, f: FileDto) => g.suggested.has(f.id);
  effective = (g: Group, f: FileDto) => this.decisions[f.relative_path] ?? this.suggested(g, f);
  isDecided = (f: FileDto) => f.relative_path in this.decisions;

  status(g: Group): Status {
    const decided = g.members.filter((m) => this.isDecided(m));
    if (decided.length === 0) return 'undecided';
    if (decided.length < g.members.length) return 'partial';
    return g.members.every((m) => this.decisions[m.relative_path] === this.suggested(g, m)) ? 'accepted' : 'changed';
  }

  statuses = $derived(new Map(this.groups.map((g) => [g.id, this.status(g)] as const)));

  private groupsByPath = $derived.by(() => {
    const m = new Map<string, Group[]>();
    for (const g of this.groups) for (const f of g.members) (m.get(f.relative_path) ?? m.set(f.relative_path, []).get(f.relative_path)!).push(g);
    return m;
  });

  /** Groups of the current tab under the current filter, in the chosen
   * date order. Undated groups go last either way. `partial` counts as
   * undecided for filtering: it still needs attention. */
  visible = $derived.by(() => {
    const dir = this.sort === 'oldest' ? 1 : -1;
    return this.groups
      .filter((g) => {
        if (g.tab !== this.tab) return false;
        const s = this.statuses.get(g.id);
        if (this.filter === 'undecided') return s === 'undecided' || s === 'partial';
        if (this.filter === 'changed') return s === 'changed';
        return true;
      })
      .sort((a, b) => {
        if (a.capturedAt === b.capturedAt) return 0; // stable: keeps engine order
        if (a.capturedAt === null) return 1;
        if (b.capturedAt === null) return -1;
        return a.capturedAt < b.capturedAt ? -dir : dir;
      });
  });

  setSort(s: Sort) {
    this.sort = s;
    try {
      localStorage.setItem(SORT_KEY, s);
    } catch {}
  }

  /** Per tab: how many groups, how many still undecided, etc. */
  countsByTab = $derived.by(() => {
    const out = Object.fromEntries(tabs.map((t) => [t, { all: 0, undecided: 0, changed: 0, reviewed: 0 }])) as Record<Tab, Counts>;
    for (const g of this.groups) {
      const c = out[g.tab];
      const s = this.statuses.get(g.id);
      c.all++;
      if (s === 'undecided' || s === 'partial') c.undecided++;
      else c.reviewed++;
      if (s === 'changed') c.changed++;
    }
    return out;
  });

  counts = $derived(this.countsByTab[this.tab]);

  /** Files the user has marked Quarantine, so far (a rough count: the exact
   * plan, with companions and refusals, comes from the server). */
  markedCount = $derived(Object.values(this.decisions).filter((keep) => keep === false).length);

  tabCount(t: Tab): number {
    return this.countsByTab[t].all;
  }

  /** The first multi-member group containing `path` that would be left with
   * nothing kept if `path` were set to `keep`, ignoring `rows`' other
   * overrides. */
  private leavesNoneKept(path: string, rows: Map<string, boolean>): Group | undefined {
    return (this.groupsByPath.get(path) ?? []).find(
      // A group of one (an orphan video) has no other copy to keep, so
      // quarantining it is the user's explicit call, not an accident.
      (g) => g.members.length > 1 && !g.members.some((m) => (m.relative_path === path ? false : (rows.get(m.relative_path) ?? this.effective(g, m)))),
    );
  }

  /** Flips one member between keep and quarantine. Refuses when that would
   * leave this group, or any other group the file is in, with no kept copy. */
  async toggle(g: Group, f: FileDto) {
    const next = !this.effective(g, f);
    if (!next) {
      const blocked = this.leavesNoneKept(f.relative_path, new Map());
      if (blocked) {
        this.error =
          blocked === g ? 'Keep at least one copy of each photo.' : `That file is the only kept copy in another group (${blocked.label}).`;
        return;
      }
    }
    await this.commit(g.members.map((m) => ({ path: m.relative_path, keep: m.id === f.id ? next : this.effective(g, m) })));
  }

  async keepAll(g: Group) {
    await this.commit(g.members.map((m) => ({ path: m.relative_path, keep: true })));
  }

  /** What accepting these groups' suggestions would write. Files already
   * marked keep stay kept, a file suggested-keep by any of the groups is
   * kept, and nothing is quarantined if that would leave some group with
   * no kept copy. */
  planAccept(groups: Group[]): { path: string; keep: boolean }[] {
    const rows = new Map<string, boolean>();
    for (const g of groups)
      for (const m of g.members) rows.set(m.relative_path, (rows.get(m.relative_path) ?? false) || g.suggested.has(m.id));
    for (const path of rows.keys()) if (this.decisions[path] === true) rows.set(path, true);

    for (let changed = true; changed; ) {
      changed = false;
      for (const [path, keep] of rows) {
        if (!keep && this.leavesNoneKept(path, rows)) {
          rows.set(path, true);
          changed = true;
        }
      }
    }
    return [...rows].map(([path, keep]) => ({ path, keep }));
  }

  async accept(groups: Group[]) {
    await this.commit(this.planAccept(groups));
  }

  /** The current tab's untouched groups, and what accepting them all would
   * move to quarantine -- shown to the user before they confirm. */
  undecidedPlan() {
    const groups = this.visibleTabGroups().filter((g) => this.statuses.get(g.id) === 'undecided');
    return { groups, quarantine: this.planAccept(groups).filter((r) => !r.keep && !this.isPathDecided(r.path)).length };
  }

  private isPathDecided = (path: string) => path in this.decisions;
  private visibleTabGroups = () => this.groups.filter((g) => g.tab === this.tab);

  async acceptAllUndecided() {
    await this.accept(this.undecidedPlan().groups);
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

const SORT_KEY = 'photodedup.sort';
function loadSort(): Sort {
  try {
    return localStorage.getItem(SORT_KEY) === 'newest' ? 'newest' : 'oldest';
  } catch {
    return 'oldest';
  }
}

const orphanReason: Record<string, string> = {
  no_match: 'No original found',
  unknown_origin: 'Source not identified',
};

/** `YYYY-MM-DD HH:MM:SS` (stored without a zone) as a readable local date. */
export function formatWhen(s: string | null): string {
  if (!s) return 'No date';
  const d = new Date(s.replace(' ', 'T'));
  return Number.isNaN(d.getTime()) ? s : d.toLocaleString(undefined, { dateStyle: 'medium', timeStyle: 'short' });
}

export const tierLabel: Record<string, string> = {
  T0: 'Identical files',
  T1: 'Same photo, different copy',
  T1b: 'Copies of the same photo',
};

export const tabInfo: Record<Tab, { title: string; blurb: string }> = {
  exact: { title: 'Exact duplicates', blurb: 'Byte-for-byte identical files. Keeping one is enough.' },
  true: {
    title: 'True duplicates',
    blurb: 'The same photo or video saved again, such as messenger copies and re-saves. The original is suggested.',
  },
  burst: {
    title: 'Quick sequences',
    blurb:
      'Photos of the same shot taken seconds apart on one camera, found by timestamp and by how alike they look (not by your camera\'s burst mode). Only the sharpest of each sequence is suggested; keep as many as you like.',
  },
  similar: {
    title: 'Same-device review',
    blurb:
      'Photos that look alike but were taken moments apart. Nothing is suggested for removal: everything is kept unless you choose otherwise. You can dismiss one of a pair, not both.',
  },
  orphans: {
    title: 'Orphan videos',
    blurb:
      'Videos that look like copies but whose original was not found, and videos whose source could not be identified so they were never compared. Everything is kept unless you choose otherwise.',
  },
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
