import './app.css';
import { mount } from 'svelte';
import App from './App.svelte';
import { applyTheme, storedTheme } from './lib/theme';
import { preloadSettings } from './lib/settings';
import { configureMedia } from './lib/api';

applyTheme(storedTheme()); // before the first paint, so there is no flash of the wrong theme

preloadSettings(); // before the first thumbnail is asked for, so Settings opens with what it needs

// The first thing asked of the server is where pictures live, so no picture is requested from the wrong
// place; it never holds the app up for long.
await Promise.race([configureMedia(), new Promise((resolve) => setTimeout(resolve, 2000))]);

export default mount(App, { target: document.getElementById('app')! });
