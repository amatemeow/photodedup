import type { KnobId, ScanInfo, Settings } from './api';

/** How each matching setting is named and written in the review. */
export const knobName: Record<KnobId, string> = {
  sequence_similarity: 'sequence similarity',
  sequence_gap_seconds: 'sequence time gap',
  match_distance: 'copy match distance',
};
export const knobUnit: Record<KnobId, string> = { sequence_similarity: '', sequence_gap_seconds: ' s', match_distance: '' };

const ids: KnobId[] = ['sequence_similarity', 'sequence_gap_seconds', 'match_distance'];

/** The matching settings that differ between what a scan used and what is
 * saved now, as readable `name: old → new` phrases. Empty when the scan
 * predates recording (nothing to compare against) or nothing changed. */
export function changedSince(used: ScanInfo['used'], now: Settings | null): string[] {
  if (!used || !now) return [];
  return ids
    .filter((k) => used[k] !== now.advanced[k].value)
    .map((k) => `${knobName[k]}: ${used[k]}${knobUnit[k]} → ${now.advanced[k].value}${knobUnit[k]}`);
}

/** Which of a scan's settings shaped what a tab shows. */
export const tabSettings: Record<string, KnobId[]> = {
  exact: [],
  true: ['match_distance'],
  burst: ['sequence_similarity', 'sequence_gap_seconds'],
  similar: ['match_distance', 'sequence_gap_seconds'],
  orphans: [],
};

export function usedLine(tab: string, used: ScanInfo['used']): string {
  const keys = tabSettings[tab] ?? [];
  if (!used || keys.length === 0) return '';
  return `Found with ${keys.map((k) => `${knobName[k]} ${used[k]}${knobUnit[k]}`).join(' and ')}.`;
}
