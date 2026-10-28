import { describe, expect, it } from 'vitest';
import type { ScanInfo, Settings } from './api';
import { changedSince, usedLine } from './knobs';

const used: NonNullable<ScanInfo['used']> = { match_distance: 45, sequence_similarity: 80, sequence_gap_seconds: 3, include_videos: true };
const knob = (value: number) => ({ value, default: value, min: 0, max: 100 });
const settings = (over: Partial<Record<'match_distance' | 'sequence_similarity' | 'sequence_gap_seconds', number>> = {}): Settings => ({
  scan_intensity: 'default',
  tiers: [],
  logical_cores: 8,
  advanced: {
    match_distance: knob(over.match_distance ?? 45),
    sequence_similarity: knob(over.sequence_similarity ?? 80),
    sequence_gap_seconds: knob(over.sequence_gap_seconds ?? 3),
  },
});

describe('settings that changed since a scan', () => {
  it('reports nothing when nothing moved', () => {
    expect(changedSince(used, settings())).toEqual([]);
  });

  it('names each setting that moved, old and new, with its unit', () => {
    expect(changedSince(used, settings({ sequence_gap_seconds: 10, match_distance: 30 }))).toEqual([
      'sequence time gap: 3 s → 10 s',
      'copy match distance: 45 → 30',
    ]);
  });

  it('says nothing for a scan that did not record its settings, or before settings load', () => {
    expect(changedSince(null, settings({ match_distance: 30 }))).toEqual([]);
    expect(changedSince(used, null)).toEqual([]);
  });

  it('describes only the settings that shaped a tab', () => {
    expect(usedLine('burst', used)).toBe('Found with sequence similarity 80 and sequence time gap 3 s.');
    expect(usedLine('true', used)).toBe('Found with copy match distance 45.');
    expect(usedLine('exact', used)).toBe('');
    expect(usedLine('orphans', used)).toBe('');
    expect(usedLine('burst', null)).toBe('');
  });
});
