import { mount } from 'svelte';

// The screen-region selection overlay (X11/Windows OCR capture) mounts INSTEAD
// of the app shell when the webview URL carries `#screen-region-overlay`
// (opened as a transparent fullscreen window by the Rust capture path). It
// must not import app.css or any store — it has to stay transparent and
// featherweight, and the app's onboarding/recovery gates must never run for
// a selection surface.
if (window.location.hash === '#screen-region-overlay') {
  const { default: ScreenRegionOverlay } = await import(
    './lib/components/ScreenRegionOverlay.svelte'
  );
  mount(ScreenRegionOverlay, {
    target: document.getElementById('app')!,
  });
} else {
  const { default: App } = await import('./App.svelte');
  await import('./app.css');
  mount(App, {
    target: document.getElementById('app')!,
  });
}
