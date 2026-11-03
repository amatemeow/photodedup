/**
 * Decides which tile pictures the browser fetches, and in what order. The browser's own lazy loading
 * starts everything in the page at once, so a screenful of slow pictures (videos, files to download)
 * takes every connection and the quick ones wait behind them. Here the number of fetches in flight is
 * limited, and each lane has its own: a video never holds up a photo. Whenever a slot frees, the
 * waiting picture nearest to where the user is looking (and likely to scroll next) goes first.
 */
export interface Job {
  /** Lower loads first; read each time a slot frees, so it follows the scrolling. */
  rank: () => number;
  /** Slow kinds (videos, cloud files) wait in a lane of their own. */
  slow: boolean;
  start: () => void;
}

export class ImageQueue {
  private active = { fast: 0, slow: 0 };
  private waiting = new Set<Job>();

  constructor(private limits = { fast: 4, slow: 2 }) {}

  /** Queues a job; the returned function must be called when the picture has loaded, failed, or is no longer wanted. */
  add(job: Job): () => void {
    let state: 'waiting' | 'running' | 'done' = 'waiting';
    const lane = job.slow ? 'slow' : 'fast';
    const wrapped: Job = {
      ...job,
      start: () => {
        state = 'running';
        this.active[lane]++;
        job.start();
      },
    };
    this.waiting.add(wrapped);
    this.pump();
    return () => {
      if (state === 'done') return;
      if (state === 'running') this.active[lane]--;
      else this.waiting.delete(wrapped);
      state = 'done';
      this.pump();
    };
  }

  /** Starts as many waiting jobs as the lanes allow, nearest first. */
  pump() {
    for (const lane of ['fast', 'slow'] as const) {
      while (this.active[lane] < this.limits[lane]) {
        let best: Job | undefined;
        let bestRank = Infinity;
        for (const j of this.waiting) {
          if (j.slow !== (lane === 'slow')) continue;
          const r = j.rank();
          if (r < bestRank) [best, bestRank] = [j, r];
        }
        if (!best) break;
        this.waiting.delete(best);
        best.start();
      }
    }
  }
}
