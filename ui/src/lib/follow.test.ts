import { describe, expect, it } from 'vitest';
import type { FileDto } from './api';
import { followGroup } from './follow';
import type { Group, Tab } from './review.svelte';

const file = (id: string) => ({ id }) as FileDto;
const group = (id: string, tab: Tab, files: string[]): Group => ({
  id,
  tab,
  label: '',
  capturedAt: null,
  reasons: [],
  members: files.map(file),
  suggested: new Set(),
  sharpness: {},
});

describe('followGroup', () => {
  it('finds the open group again after every group got a new id', () => {
    const after = [group('n1', 'true', ['x', 'y']), group('n2', 'true', ['a', 'b', 'c'])];
    expect(followGroup({ tab: 'true', fileIds: ['a', 'b', 'c'] }, after)?.id).toBe('n2');
  });

  it('follows the larger part when a group splits', () => {
    const after = [group('big', 'true', ['a', 'b', 'c']), group('small', 'true', ['d', 'e'])];
    expect(followGroup({ tab: 'true', fileIds: ['a', 'b', 'c', 'd', 'e'] }, after)?.id).toBe('big');
  });

  it('follows the merged group when several merge into one', () => {
    const after = [group('merged', 'true', ['a', 'b', 'c', 'd'])];
    expect(followGroup({ tab: 'true', fileIds: ['a', 'b'] }, after)?.id).toBe('merged');
  });

  it('follows a group into another category', () => {
    const after = [group('moved', 'burst', ['a', 'b'])];
    const found = followGroup({ tab: 'similar', fileIds: ['a', 'b'] }, after);
    expect(found?.id).toBe('moved');
    expect(found?.tab).toBe('burst');
  });

  it('prefers the same category, then the closest size, when overlaps tie', () => {
    const open = { tab: 'true' as Tab, fileIds: ['a', 'b'] };
    expect(followGroup(open, [group('other', 'burst', ['a', 'z']), group('same', 'true', ['b', 'y', 'w'])])?.id).toBe('same');
    expect(followGroup(open, [group('three', 'true', ['a', 'p', 'q']), group('two', 'true', ['b', 'r'])])?.id).toBe('two');
  });

  it('is null when none of its photos is in a group any more', () => {
    expect(followGroup({ tab: 'true', fileIds: ['a', 'b'] }, [group('n', 'true', ['x', 'y'])])).toBeNull();
    expect(followGroup({ tab: 'true', fileIds: ['a'] }, [])).toBeNull();
  });
});
