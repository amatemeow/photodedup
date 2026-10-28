import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { Cluster, FileDto } from './api';

// The backend, faked in memory: what matters here is the client-side
// decision logic, not HTTP.
const server = vi.hoisted(() => ({
  decisions: {} as Record<string, boolean>,
  failNextWrite: false,
}));

vi.mock('./api', () => ({
  api: {
    clusters: async () => clusters,
    decisions: async () => ({ ...server.decisions }),
    putDecisions: async (_: number, rows: { relative_path: string; keep: boolean }[]) => {
      if (server.failNextWrite) {
        server.failNextWrite = false;
        throw new Error('disk full');
      }
      for (const r of rows) server.decisions[r.relative_path] = r.keep;
    },
    clearDecision: async (_: number, p: string) => {
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
// Cluster 1 has three members (multi-keep case); cluster 2 has two.
const clusters: Cluster[] = [
  { id: 'c1', tier: 'T1', keeper_file_id: 'f10', reasons: [], members: [file('f10', 'a.jpg'), file('f11', 'b.jpg'), file('f12', 'c.jpg')] },
  { id: 'c2', tier: 'T0', keeper_file_id: 'f20', reasons: [], members: [file('f20', 'd.jpg'), file('f21', 'e.jpg')] },
];

async function loaded() {
  const { Review } = await import('./review.svelte');
  const r = new Review('scan-1');
  await r.load();
  return r;
}

beforeEach(() => {
  server.decisions = {};
  server.failNextWrite = false;
});

describe('Review', () => {
  it('starts undecided, with the engine pick only a suggestion', async () => {
    const r = await loaded();
    expect(r.statuses.get('c1')).toBe('undecided');
    expect(r.counts).toMatchObject({ all: 2, undecided: 2, changed: 0, reviewed: 0 });
    expect(Object.keys(server.decisions)).toHaveLength(0);
    expect(r.effective(clusters[0], clusters[0].members[0])).toBe(true);
    expect(r.effective(clusters[0], clusters[0].members[1])).toBe(false);
  });

  it('accepting writes every member of the cluster and marks it accepted', async () => {
    const r = await loaded();
    await r.accept([clusters[0]]);
    expect(server.decisions).toEqual({ 'a.jpg': true, 'b.jpg': false, 'c.jpg': false });
    expect(r.statuses.get('c1')).toBe('accepted');
    expect(r.counts.reviewed).toBe(1);
  });

  it('allows keeping more than one member, and calls that changed', async () => {
    const r = await loaded();
    await r.toggle(clusters[0], clusters[0].members[1]); // also keep b.jpg
    expect(server.decisions).toEqual({ 'a.jpg': true, 'b.jpg': true, 'c.jpg': false });
    expect(r.statuses.get('c1')).toBe('changed');
    expect(r.counts.changed).toBe(1);
  });

  it('refuses to quarantine the last kept member', async () => {
    const r = await loaded();
    await r.toggle(clusters[1], clusters[1].members[0]); // would leave nothing kept
    expect(r.error).toMatch(/at least one/i);
    expect(server.decisions).toEqual({});
  });

  it('undo restores earlier values, and returns first decisions to undecided', async () => {
    const r = await loaded();
    await r.accept([clusters[0]]);
    await r.toggle(clusters[0], clusters[0].members[1]);
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
    await r.accept([clusters[0]]);
    expect(r.error).toMatch(/disk full/);
    expect(r.statuses.get('c1')).toBe('undecided');
    expect(r.canUndo).toBe(false);
  });

  it('accept-all only touches clusters nobody has decided yet', async () => {
    const r = await loaded();
    await r.toggle(clusters[0], clusters[0].members[1]); // user's own choice
    await r.acceptAllUndecided();
    expect(r.statuses.get('c1')).toBe('changed'); // left alone
    expect(r.statuses.get('c2')).toBe('accepted');
  });

  it('treats a half-written cluster as partial and still needing attention', async () => {
    server.decisions = { 'a.jpg': true };
    const r = await loaded();
    expect(r.statuses.get('c1')).toBe('partial');
    r.filter = 'undecided';
    expect(r.visible.map((c) => c.id)).toEqual(['c1', 'c2']);
  });

  it('filters to changed clusters only', async () => {
    const r = await loaded();
    await r.accept([clusters[1]]);
    await r.toggle(clusters[0], clusters[0].members[1]);
    r.filter = 'changed';
    expect(r.visible.map((c) => c.id)).toEqual(['c1']);
  });
});
