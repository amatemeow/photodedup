import { describe, expect, it } from 'vitest';
import { cloudLine } from './cloud';

const bytes = (n: number) => `${(n / 1e9).toFixed(1)} GB`;

describe('cloudLine', () => {
  it('says what is being waited for, with files and size', () => {
    const line = cloudLine({ done: 16, total: 87, bytes_done: 0.9e9, bytes_total: 7.5e9 }, bytes);
    expect(line).toContain('16 of 87 online-only files');
    expect(line).toContain('0.9 GB of 7.5 GB');
    expect(line).toMatch(/waits for your cloud storage/);
  });

  it('uses the singular for one file', () => {
    expect(cloudLine({ done: 0, total: 1, bytes_done: 0, bytes_total: 5e6 }, bytes)).toContain('0 of 1 online-only file ');
  });

  it('is empty when nothing is online-only', () => {
    expect(cloudLine(undefined, bytes)).toBe('');
    expect(cloudLine({ done: 0, total: 0, bytes_done: 0, bytes_total: 0 }, bytes)).toBe('');
  });
});
