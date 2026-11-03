import { describe, expect, it } from 'vitest';
import { ImageQueue } from './imageQueue';

describe('ImageQueue', () => {
  const job = (log: string[], name: string, rank: number, slow = false) => ({ rank: () => rank, slow, start: () => log.push(name) });

  it('starts the nearest waiting picture first when a slot frees', () => {
    const q = new ImageQueue({ fast: 1, slow: 1 });
    const log: string[] = [];
    const first = q.add(job(log, 'a', 5));
    q.add(job(log, 'far', 9));
    q.add(job(log, 'near', 1));
    first();
    expect(log).toEqual(['a', 'near']);
  });

  it('keeps slow pictures from holding up quick ones', () => {
    const q = new ImageQueue({ fast: 1, slow: 1 });
    const log: string[] = [];
    q.add(job(log, 'video1', 0, true));
    q.add(job(log, 'video2', 0, true));
    q.add(job(log, 'photo', 3));
    expect(log).toEqual(['video1', 'photo']);
  });

  it('drops a picture that is no longer wanted before it starts', () => {
    const q = new ImageQueue({ fast: 1, slow: 1 });
    const log: string[] = [];
    const a = q.add(job(log, 'a', 0));
    const gone = q.add(job(log, 'gone', 1));
    gone();
    a();
    expect(log).toEqual(['a']);
  });
});
