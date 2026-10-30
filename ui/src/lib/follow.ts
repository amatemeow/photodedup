import type { Group, Tab } from './review.svelte';

/** What was open before the matching settings regrouped the scan. */
export interface OpenGroup {
  tab: Tab;
  fileIds: string[];
}

/**
 * Regrouping gives every group a new id, so the group someone had open can't
 * be found by id afterwards. Its photos are still there, though: the new group
 * that holds most of them is the same thing to look at, however it was merged
 * or split. On a tie the one in the same category wins, then the one closest
 * in size. `null` means none of its photos is in any group any more (for
 * example it stopped being a duplicate), and the view should go back to the
 * overview.
 */
export function followGroup(before: OpenGroup, groups: Group[]): Group | null {
  const was = new Set(before.fileIds);
  let best: Group | null = null;
  let bestOverlap = 0;
  const better = (g: Group, overlap: number) => {
    if (!best) return true;
    if (overlap !== bestOverlap) return overlap > bestOverlap;
    if ((g.tab === before.tab) !== (best.tab === before.tab)) return g.tab === before.tab;
    return Math.abs(g.members.length - was.size) < Math.abs(best.members.length - was.size);
  };
  for (const g of groups) {
    const overlap = g.members.reduce((n, m) => n + (was.has(m.id) ? 1 : 0), 0);
    if (overlap > 0 && better(g, overlap)) {
      best = g;
      bestOverlap = overlap;
    }
  }
  return best;
}
