// Light / dark appearance: follow the system by default, or pin one. Kept in
// localStorage (a per-device display preference, not part of the scan data).
export type Theme = 'system' | 'light' | 'dark';
const KEY = 'photodedup.theme';

export function storedTheme(): Theme {
  try {
    const v = localStorage.getItem(KEY);
    return v === 'light' || v === 'dark' ? v : 'system';
  } catch {
    return 'system';
  }
}

/** Pins the theme on <html>; app.css reads `data-theme`, and with none set follows the system. */
export function applyTheme(t: Theme) {
  const root = document.documentElement;
  if (t === 'system') root.removeAttribute('data-theme');
  else root.setAttribute('data-theme', t);
  try {
    if (t === 'system') localStorage.removeItem(KEY);
    else localStorage.setItem(KEY, t);
  } catch {
    // private mode: the choice lasts until the window closes
  }
}
