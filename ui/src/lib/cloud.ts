import type { CloudProgress } from './api';

/**
 * What to tell someone while a scan waits for online-only files to download.
 * Reading such a file makes the sync client (OneDrive, iCloud, ...) fetch it
 * first, so the scan waits on the network and the computer looks idle; saying so
 * keeps it from looking stuck. Empty when nothing is online-only.
 */
export function cloudLine(c: CloudProgress | undefined, formatBytes: (n: number) => string): string {
  if (!c || c.total <= 0) return '';
  const files = `${c.done} of ${c.total} online-only ${c.total === 1 ? 'file' : 'files'}`;
  return `Downloading ${files} (${formatBytes(c.bytes_done)} of ${formatBytes(c.bytes_total)}). The scan waits for your cloud storage, so the computer looks idle.`;
}
