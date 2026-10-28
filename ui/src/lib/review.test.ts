import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { BurstRun, Cluster, FileDto, ReviewPair } from './api';

// The backend, faked in memory: what matters here is the client-side
// decision logic, not HTTP.
const server = vi.hoisted(() => ({
  decisions: {} as Record<string, boolean>,
  failNextWrite: false,
  clusters: [] as Cluster[],
  bursts: [] as BurstRun[],
  pairs: [] as ReviewPair[],
}));

vi.mock('./api', () => ({
  api: {
    clusters: async () => server.clusters,
    burstRuns: async () => server.bursts,
    reviewPairs: async () => server.pairs,
    orphanVideos: async () => [],
    decisions: async () => ({ ...server.decisions }),
    putDecisions: async (_: string, rows: { relative_path: string; keep: boolean }[]) => {
      if (server.failNextWrite) {
        server.failNextWrite = false;
        throw new Error('disk full');
      }
      for (const r of rows) server.decisions[r.relative_path] = r.keep;
    },
    clearDecision: async (_: string, p: string) => {
      delete server.decisions[p];
    },
    analysis: async () => null,
  },
}));

const file = (id: string, path: string): FileDto => ({
  id,
  relative_path: path,
  file_name: path,
  media_class: 'Image',
  size_bytes: 1,
  is_placeholder: false,
});
const cluster = (id: string, tier: string, keeper: string, members: FileDto[]): Cluster => ({
  id,
  tier,
  keeper_file_id: keeper,
  reasons: [],
  members,
});

const A = file('fa', 'a.jpg');
const B = file('fb', 'b.jpg');
const C = file('fc', 'c.jpg');
const D = file('fd', 'd.jpg');
const E = file('fe', 'e.jpg');

function seed() {
  server.decisions = {};
  server.failNextWrite = false;
  // c1: three members (multi-keep case); c2: exact pair.
  server.clusters = [cluster('c1', 'T1', 'fa', [A, B, C]), cluster('c2', 'T0', 'fd', [D, E])];
  server.bursts = [];
  server.pairs = [];
}

async function loaded() {
  const { Review } = await import('./review.svelte');
  const r = new Review('scan-1');
  await r.load();
  return r;
}
const group = (r: Awaited<ReturnType<typeof loaded>>, id: string) => r.groups.find((g) => g.id === id)!;

beforeEach(seed);

describe('categories', () => {
  it('puts exact copies, true duplicates and bursts in their own tabs, with per-tab counts', async () => {
    server.bursts = [{ id: 'b1', camera_model: 'Cam', frames: [file('f1', '1.jpg'), file('f2', '2.jpg')], sharpness: { f1: 5, f2: 9 }, suggested_file_id: 'f2' }];
    const r = await loaded();
    expect(r.groups.filter((g) => g.tab === 'exact').map((g) => g.id)).toEqual(['c2']);
    expect(r.groups.filter((g) => g.tab === 'true').map((g) => g.id)).toEqual(['c1']);
    expect(r.groups.filter((g) => g.tab === 'burst').map((g) => g.id)).toEqual(['b1']);
    expect(r.countsByTab.exact.all).toBe(1);
    expect(r.countsByTab.burst.undecided).toBe(1);
  });

  it('opens on the first category that has something in it', async () => {
    server.clusters = [cluster('c1', 'T1', 'fa', [A, B])];
    expect((await loaded()).tab).toBe('true');
    server.clusters = [];
    server.bursts = [{ id: 'b1', camera_model: 'Cam', frames: [A, B], sharpness: {}, suggested_file_id: 'fa' }];
    expect((await loaded()).tab).toBe('burst');
  });

  it('suggests only the sharpest frame of a burst, but keeps a frame that is a duplicate group\'s keeper', async () => {
    const f1 = file('f1', '1.jpg');
    const f2 = file('f2', '2.jpg');
    const f3 = file('f3', '3.jpg');
    server.clusters = [cluster('c1', 'T1', 'f1', [f1, A])]; // f1 is c1's suggested keeper
    server.bursts = [{ id: 'b1', camera_model: 'Cam', frames: [f1, f2, f3], sharpness: { f1: 1, f2: 9, f3: 4 }, suggested_file_id: 'f2' }];
    const r = await loaded();
    expect([...group(r, 'b1').suggested].sort()).toEqual(['f1', 'f2']);
  });
});

describe('browse-only tabs', () => {
  it('leaves burst-tier pairs to the Burst tab, so Same-device review counts only the others', async () => {
    const pair = (id: string, tier: string): ReviewPair => ({ id, tier, reason: 'r', phash_dist: 30, file_a: A, file_b: B });
    server.pairs = [pair('p1', 'T2Burst'), pair('p2', 'T2NativeReview'), pair('p3', 'T2Burst'), pair('p4', 'T3CrossDevice')];
    const r = await loaded();
    expect(r.pairs.map((p) => p.id)).toEqual(['p2', 'p4']);
    expect(r.tabCount('similar')).toBe(2);
  });
});

describe('deciding', () => {
  it('starts undecided, with the engine pick only a suggestion', async () => {
    const r = await loaded();
    const c1 = group(r, 'c1');
    expect(r.statuses.get('c1')).toBe('undecided');
    expect(Object.keys(server.decisions)).toHaveLength(0);
    expect(r.effective(c1, A)).toBe(true);
    expect(r.effective(c1, B)).toBe(false);
  });

  it('accepting writes every member and marks the group accepted', async () => {
    const r = await loaded();
    await r.accept([group(r, 'c1')]);
    expect(server.decisions).toEqual({ 'a.jpg': true, 'b.jpg': false, 'c.jpg': false });
    expect(r.statuses.get('c1')).toBe('accepted');
  });

  it('allows keeping several members; the group counts as changed', async () => {
    const r = await loaded();
    await r.toggle(group(r, 'c1'), B);
    expect(server.decisions).toEqual({ 'a.jpg': true, 'b.jpg': true, 'c.jpg': false });
    expect(r.statuses.get('c1')).toBe('changed');
    expect(r.countsByTab.true.changed).toBe(1);
  });

  it('never leaves a group with nothing kept', async () => {
    const r = await loaded();
    await r.toggle(group(r, 'c2'), D); // D is the only kept member
    expect(r.error).toMatch(/at least one/i);
    expect(server.decisions).toEqual({});
  });

  it('refuses to quarantine a file that is the only kept copy in a different group', async () => {
    // x is in both clusters; in cB it is the suggested keeper (z is not).
    const x = file('fx', 'x.jpg');
    const y = file('fy', 'y.jpg');
    const z = file('fz', 'z.jpg');
    server.clusters = [cluster('cA', 'T1', 'fy', [x, y]), cluster('cB', 'T1', 'fx', [x, z])];
    const r = await loaded();

    await r.toggle(group(r, 'cA'), x); // keep x in cA as well: fine
    expect(r.error).toBe('');
    const before = { ...server.decisions };

    await r.toggle(group(r, 'cA'), x); // quarantine x: cA still has y, but cB would have nothing
    expect(r.error).toMatch(/only kept copy in another group/i);
    expect(server.decisions).toEqual(before);
  });

  it('bulk accept never quarantines a file that another group still needs', async () => {
    // y is c-A's non-keeper but c-B's keeper: accepting A alone must not quarantine y.
    const x = file('fx', 'x.jpg');
    const y = file('fy', 'y.jpg');
    const z = file('fz', 'z.jpg');
    server.clusters = [cluster('cA', 'T1', 'fx', [x, y]), cluster('cB', 'T1', 'fy', [y, z])];
    const r = await loaded();
    await r.accept([group(r, 'cA')]);
    expect(server.decisions['y.jpg']).toBe(true);
    expect(server.decisions['x.jpg']).toBe(true);
  });

  it('bulk accept never turns an explicit keep into a quarantine', async () => {
    const r = await loaded();
    await r.toggle(group(r, 'c1'), B); // user keeps B as well
    await r.accept([group(r, 'c1')]); // suggestion would quarantine B
    expect(server.decisions['b.jpg']).toBe(true);
  });

  it('keep-all marks every frame kept', async () => {
    server.bursts = [{ id: 'b1', camera_model: 'Cam', frames: [A, B, C], sharpness: {}, suggested_file_id: 'fa' }];
    server.clusters = [];
    const r = await loaded();
    await r.keepAll(group(r, 'b1'));
    expect(server.decisions).toEqual({ 'a.jpg': true, 'b.jpg': true, 'c.jpg': true });
    expect(r.statuses.get('b1')).toBe('changed');
  });

  it('the accept-all preview counts what would go to quarantine, in the current tab only', async () => {
    const r = await loaded();
    r.tab = 'true';
    expect(r.undecidedPlan().quarantine).toBe(2); // b.jpg, c.jpg
    r.tab = 'exact';
    expect(r.undecidedPlan().quarantine).toBe(1); // e.jpg
  });

  it('accept-all only touches groups nobody has decided yet', async () => {
    const r = await loaded();
    r.tab = 'true';
    await r.toggle(group(r, 'c1'), B);
    await r.acceptAllUndecided();
    expect(r.statuses.get('c1')).toBe('changed');
    expect(r.statuses.get('c2')).toBe('undecided'); // other tab, untouched
  });
});

describe('undo and failure', () => {
  it('undo restores earlier values, and returns first decisions to undecided', async () => {
    const r = await loaded();
    await r.accept([group(r, 'c1')]);
    await r.toggle(group(r, 'c1'), B);
    expect(r.statuses.get('c1')).toBe('changed');
    await r.undo();
    expect(r.statuses.get('c1')).toBe('accepted');
    await r.undo();
    expect(r.statuses.get('c1')).toBe('undecided');
    expect(server.decisions).toEqual({});
    expect(r.canUndo).toBe(false);
  });

  it('rolls the screen back when the server refuses a write', async () => {
    const r = await loaded();
    server.failNextWrite = true;
    await r.accept([group(r, 'c1')]);
    expect(r.error).toMatch(/disk full/);
    expect(r.statuses.get('c1')).toBe('undecided');
    expect(r.canUndo).toBe(false);
  });

  it('treats a half-written group as partial, still needing attention', async () => {
    server.decisions = { 'a.jpg': true };
    const r = await loaded();
    expect(r.statuses.get('c1')).toBe('partial');
    r.tab = 'true';
    r.filter = 'undecided';
    expect(r.visible.map((g) => g.id)).toEqual(['c1']);
  });

  it('filters to changed groups only', async () => {
    const r = await loaded();
    r.tab = 'true';
    await r.accept([group(r, 'c2')]);
    await r.toggle(group(r, 'c1'), B);
    r.filter = 'changed';
    expect(r.visible.map((g) => g.id)).toEqual(['c1']);
  });
});
