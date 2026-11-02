import { api, type Settings } from './api';

/** The last settings the server gave us. Kept so Settings can open at once with what was known, and be
 * refreshed behind it, instead of waiting for the server (which can be busy rendering previews). */
export const known: { value: Settings | null } = { value: null };

/** Reads the settings and remembers them. Started when the app starts, before anything else competes for
 * the connection. */
export async function refreshSettings(): Promise<Settings> {
  const s = await api.settings();
  known.value = s;
  return s;
}

export const preloadSettings = () => {
  refreshSettings().catch(() => {});
};
