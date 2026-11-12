import './splash.css';
import { invoke } from '@tauri-apps/api/core';

// The splash ends when the last animation (the wordmark) really finishes, plus a short hold, so a slow
// first paint (WebView2 warming up) never cuts the animation short. Nominal length is about 2.4 s.
const HOLD_MS = 350;
const REDUCED_MS = 800;
const HARD_CAP_MS = 5000; // slow systems: never stay longer than this
const reduced = window.matchMedia('(prefers-reduced-motion: reduce)').matches;
let finished = false;

function finish() {
  if (finished) return;
  finished = true;
  document.getElementById('stage')?.classList.add('leaving');
  // Let the 180 ms fade finish, then ask the backend to close us and reveal the main window.
  setTimeout(() => { void invoke('splash_finished').catch(() => undefined); }, 190);
}

// Never block startup: any error ends the splash immediately.
window.addEventListener('error', finish);
window.addEventListener('unhandledrejection', finish);
setTimeout(finish, HARD_CAP_MS);
if (reduced) {
  setTimeout(finish, REDUCED_MS);
} else {
  document.querySelector('.wordmark')?.addEventListener('animationend', () => setTimeout(finish, HOLD_MS), { once: true });
}
