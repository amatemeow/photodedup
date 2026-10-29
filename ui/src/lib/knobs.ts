import type { Counts, KnobId, ScanInfo, Settings } from './api';

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

const plural = (n: number, one: string, many: string) => `${n} ${n === 1 ? one : many}`;

/** What moving a setting does to the lists it feeds, as readable lines.
 * Only the lists that setting can affect are mentioned, and only when they
 * would differ. Empty when nothing would change. */
export function describeChange(knob: KnobId, from: Counts, to: Counts): string[] {
  const lines: string[] = [];
  if (knob !== 'match_distance' && (from.sequences !== to.sequences || from.sequence_photos !== to.sequence_photos)) {
    lines.push(
      `Quick sequences: ${from.sequences} → ${to.sequences} (${plural(from.sequence_photos, 'photo', 'photos')} → ${to.sequence_photos})`,
    );
  }
  if (knob === 'match_distance' && from.duplicates !== to.duplicates) {
    lines.push(`True duplicates: ${from.duplicates} → ${to.duplicates}`);
  }
  if (knob !== 'sequence_similarity' && from.look_alikes !== to.look_alikes) {
    lines.push(`Look-alike pairs: ${from.look_alikes} → ${to.look_alikes}`);
  }
  return lines;
}
