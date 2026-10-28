import './app.css';
import { mount } from 'svelte';
import App from './App.svelte';
import { applyTheme, storedTheme } from './lib/theme';

applyTheme(storedTheme()); // before the first paint, so there is no flash of the wrong theme

export default mount(App, { target: document.getElementById('app')! });
