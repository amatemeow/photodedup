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
/** Photos and videos are reviewed apart: bulk actions only ever reach the
 * kind on screen, so a video is never swept in by a click made on photos. */
export type Media = 'photo' | 'video';
export const mediaKinds: Media[] = ['photo', 'video'];

/** The three shapes a card takes in the overview, by the shape of its photo. */
export type Shape = 'portrait' | 'landscape' | 'square';
export const shapeOfRatio = (r: number): Shape => (r > 1.15 ? 'landscape' : r < 0.87 ? 'portrait' : 'square');
/** `measured`: shapes read off loaded thumbnails, for files whose size the scan did not record. */
export function shapeOf(g: Group, measured: Record<string, Shape> = {}): Shape {
  if (g.tab === 'similar') return 'landscape'; // a pair: two photos side by side
  const m = g.members.find((f) => f.width && f.height);
  if (m) return shapeOfRatio(m.width! / m.height!);
  return g.members.map((f) => measured[f.id]).find(Boolean) ?? 'landscape';
}
export const mediaOf = (g: { members: { media_class: string }[] }): Media =>
  g.members.length > 0 && g.members.every((m) => m.media_class === 'Video') ? 'video' : 'photo';

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
 * A person may keep none of a group (a warning says so); a bulk action never
 * empties a group on its own, and never quarantines a file marked keep. */
export class Review {
  groups = $state.raw<Group[]>([]);
  sort = $state<Sort>(loadSort());
  decisions = $state<Record<string, boolean>>({});
  tab = $state<Tab>('true');
  media = $state<Media>('photo');
  filter = $state<Filter>('all');
  loaded = $state(false);
  /** Counts the (re)loads, so a view that lists files knows when to fetch its list again. */
  version = $state(0);
  error = $state('');
  canUndo = $state(false);

  private undoStack: UndoEntry[] = [];
  private analyses = new Map<string, Promise<Analysis | null>>();

  constructor(readonly scanId: string) {}

  /** `keepView`: reloading under the user (their tab and filter stay), as when matching
   * settings are being tuned; the first load opens on the first category with anything in it. */
  async load(keepView = false) {
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
              label: reasonText(p.reason),
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
      if (!keepView) {
        this.media = this.mediaCounts.photo.all > 0 || this.mediaCounts.video.all === 0 ? 'photo' : 'video';
        this.tab = tabs.find((t) => this.tabCount(t) > 0) ?? 'true';
      }
      this.loaded = true;
      this.version++;
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
        if (g.tab !== this.tab || mediaOf(g) !== this.media) return false;
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
      if (mediaOf(g) !== this.media) continue;
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

  /** Per kind, across all categories: what the Photos / Videos switch shows. */
  mediaCounts = $derived.by(() => {
    const out: Record<Media, { all: number; undecided: number; reviewed: number }> = {
      photo: { all: 0, undecided: 0, reviewed: 0 },
      video: { all: 0, undecided: 0, reviewed: 0 },
    };
    for (const g of this.groups) {
      const c = out[mediaOf(g)];
      const s = this.statuses.get(g.id);
      c.all++;
      if (s === 'undecided' || s === 'partial') c.undecided++;
      else c.reviewed++;
    }
    return out;
  });

  /** Files the user has marked Quarantine, so far (a rough count: the exact
   * plan, with companions and refusals, comes from the server). */
  markedCount = $derived(Object.values(this.decisions).filter((keep) => keep === false).length);

  tabCount(t: Tab): number {
    return this.countsByTab[t].all;
  }

  /** True when every member of a group of two or more is marked quarantine. Allowed,
   * since the user may want all of them gone, but shown as a warning. A lone file
   * (an orphan video) has no other copy to compare, so it never counts. */
  keepsNone = (g: Group): boolean => g.members.length > 1 && g.members.every((m) => !this.effective(g, m));

  /** How many groups in view would lose every member: what the Apply confirmation warns about. */
  emptiedCount = $derived(this.groups.filter((g) => this.keepsNone(g)).length);

  /** The first multi-member group containing `path` that would be left with
   * nothing kept if `path` were set to `keep`, ignoring `rows`' other
   * overrides. Only bulk "accept" uses this: a click on a single file may
   * leave a group empty, but accepting many suggestions at once never does
   * it behind the user's back. */
  private leavesNoneKept(path: string, rows: Map<string, boolean>): Group | undefined {
    return (this.groupsByPath.get(path) ?? []).find(
      (g) => g.members.length > 1 && !g.members.some((m) => (m.relative_path === path ? false : (rows.get(m.relative_path) ?? this.effective(g, m)))),
    );
  }

  /** Flips one member between keep and quarantine. */
  async toggle(g: Group, f: FileDto) {
    const next = !this.effective(g, f);
    await this.commit(g.members.map((m) => ({ path: m.relative_path, keep: m.id === f.id ? next : this.effective(g, m) })));
  }

  /** The groups a file is in (a file can be in more than one), for the library overview. */
  groupsFor = (path: string): Group[] => this.groupsByPath.get(path) ?? [];

  /** Marks single files Quarantine from the library overview, or clears the mark. Such a mark
   * counts for a file that is in no group too. Undoable like any other choice. */
  async markLibrary(paths: string[], quarantine: boolean) {
    if (!quarantine) {
      const had = paths.filter((p) => p in this.decisions);
      if (had.length === 0) return;
      this.error = '';
      const prev = had.map((path) => ({ path, keep: this.decisions[path] }));
      for (const p of had) delete this.decisions[p];
      try {
        await api.clearDecisions(this.scanId, had);
        this.undoStack.push({ prev });
        this.canUndo = true;
      } catch (e) {
        for (const p of prev) this.decisions[p.path] = p.keep!;
        this.error = `Couldn't save: ${e instanceof Error ? e.message : e}`;
      }
      return;
    }
    await this.commit(paths.map((path) => ({ path, keep: false })), true);
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
    const paths = new Set(this.planAccept(groups).filter((r) => !r.keep && !this.isPathDecided(r.path)).map((r) => r.path));
    const files = groups.flatMap((g) => g.members).filter((m, i, all) => paths.has(m.relative_path) && all.findIndex((o) => o.id === m.id) === i);
    return { groups, quarantine: files.length, files, bytes: files.reduce((n, f) => n + f.size_bytes, 0) };
  }

  private isPathDecided = (path: string) => path in this.decisions;
  private visibleTabGroups = () => this.groups.filter((g) => g.tab === this.tab && mediaOf(g) === this.media);

  async acceptAllUndecided() {
    await this.accept(this.undecidedPlan().groups);
  }

  /** The files in these groups that have a choice recorded: what resetting them forgets. */
  resetPaths(groups: Group[]): string[] {
    return [...new Set(groups.flatMap((g) => g.members.map((m) => m.relative_path)))].filter((p) => p in this.decisions);
  }

  /** Forgets the choices on these groups' files, so each goes back to the
   * engine's suggestion and the group to "to review". A file that is in
   * another group too has one choice, so it resets there as well. Undoable. */
  async reset(groups: Group[]) {
    const paths = this.resetPaths(groups);
    if (paths.length === 0) return;
    this.error = '';
    const prev = paths.map((path) => ({ path, keep: this.decisions[path] }));
    for (const p of paths) delete this.decisions[p];
    try {
      await api.clearDecisions(this.scanId, paths);
      this.undoStack.push({ prev });
      this.canUndo = true;
    } catch (e) {
      for (const p of prev) this.decisions[p.path] = p.keep!;
      this.error = `Couldn't reset: ${e instanceof Error ? e.message : e}`;
    }
  }

  /** The other kind's groups still waiting, so a bulk action can say they are untouched. */
  otherWaiting = $derived(this.mediaCounts[this.media === 'photo' ? 'video' : 'photo'].undecided);

  /** The on-screen kind's groups in one category, or in every category. */
  groupsOf = (scope: 'tab' | 'all'): Group[] =>
    this.groups.filter((g) => mediaOf(g) === this.media && (scope === 'all' || g.tab === this.tab));

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
  private async commit(rows: { path: string; keep: boolean }[], manual = false) {
    this.error = '';
    const prev = rows.map((r) => ({ path: r.path, keep: this.decisions[r.path] }));
    for (const r of rows) this.decisions[r.path] = r.keep;
    try {
      await api.putDecisions(this.scanId, rows.map((r) => ({ relative_path: r.path, keep: r.keep })), manual);
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

/** The engine's reason codes, in words a person would use. A code with no entry
 * here (a newer engine) still reads as words, not as an identifier. */
const reasonWords: Record<string, string> = {
  native_vs_derived: 'An original and a copy of it',
  derived_vs_derived_no_surviving_original: 'Copies of one photo, original gone',
  derived_vs_derived_too_far_apart: 'Copies that look alike but were saved at different times',
  native_native_review: 'Look alike, from the same camera',
  native_native_cross_device: 'Look alike, from different cameras',
  unknown_provenance_review: 'Look alike, where one came from is unclear',
  burst_same_camera: 'A quick sequence from one camera',
};
export function reasonText(code: string): string {
  const known = reasonWords[code];
  if (known) return known;
  const words = code.replaceAll('_', ' ').trim();
  return words ? words[0].toUpperCase() + words.slice(1) : '';
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
      'Photos that look alike but were taken moments apart. Nothing is suggested for removal: everything is kept unless you choose otherwise. You can dismiss either photo of a pair, or both.',
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
