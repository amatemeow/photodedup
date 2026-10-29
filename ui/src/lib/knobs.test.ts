import { describe, expect, it } from 'vitest';
import type { Counts, ScanInfo, Settings } from './api';
import { cacheStopIndex, cacheStops, changedSince, describeChange, sizeLabel, usedLine } from './knobs';

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
  cache: { budget_mb: knob(1024), used_bytes: 0 },
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

const counts = (over: Partial<Counts> = {}): Counts => ({ exact: 2, duplicates: 180, sequences: 4, sequence_photos: 14, look_alikes: 9, ...over });

describe('describing what a setting would change', () => {
  it('says nothing when the lists it feeds would stay the same', () => {
    expect(describeChange('sequence_gap_seconds', counts(), counts())).toEqual([]);
  });

  it('reports sequences with their photo count, and only for settings that affect them', () => {
    const to = counts({ sequences: 3, sequence_photos: 11 });
    expect(describeChange('sequence_similarity', counts(), to)).toEqual(['Quick sequences: 4 → 3 (14 photos → 11)']);
    expect(describeChange('match_distance', counts(), to)).toEqual([]);
  });

  it('the time gap moves pairs between sequences and look-alikes, so reports both', () => {
    const to = counts({ sequences: 5, sequence_photos: 20, look_alikes: 4 });
    expect(describeChange('sequence_gap_seconds', counts(), to)).toEqual([
      'Quick sequences: 4 → 5 (14 photos → 20)',
      'Look-alike pairs: 9 → 4',
    ]);
  });

  it('copy match distance reports duplicates and look-alikes but never sequences', () => {
    const to = counts({ duplicates: 190, look_alikes: 14, sequences: 9 });
    expect(describeChange('match_distance', counts(), to)).toEqual(['True duplicates: 180 → 190', 'Look-alike pairs: 9 → 14']);
  });

  it('uses the singular for one photo', () => {
    expect(describeChange('sequence_gap_seconds', counts({ sequence_photos: 1 }), counts({ sequences: 5, sequence_photos: 1 }))[0]).toBe(
      'Quick sequences: 4 → 5 (1 photo → 1)',
    );
  });
});

describe('the preview cache size slider', () => {
  it('has stops that run from the smallest to the largest allowed size', () => {
    expect(cacheStops[0]).toBe(256);
    expect(cacheStops.at(-1)).toBe(10240);
    expect([...cacheStops].sort((a, b) => a - b)).toEqual(cacheStops);
  });

  it('puts a saved value on its own stop, or the nearest one below it', () => {
    expect(cacheStopIndex(1024)).toBe(2);
    expect(cacheStopIndex(1500)).toBe(2);
    expect(cacheStopIndex(10)).toBe(0);
  });

  it('writes sizes the way a person would', () => {
    expect(sizeLabel(512)).toBe('512 B');
    expect(sizeLabel(300 * 1024)).toBe('300 KB');
    expect(sizeLabel(312 * 1024 * 1024)).toBe('312 MB');
    expect(sizeLabel(1024 ** 3)).toBe('1 GB');
    expect(sizeLabel(2.5 * 1024 ** 3)).toBe('2.5 GB');
  });
});
