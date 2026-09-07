import { mount } from 'svelte';

// Hash-routed special pages mount INSTEAD of the app shell, and each route
// must import ONLY its own component. These pages load no app.css, so their
// components carry page-global styles — ScreenRegionOverlay forces
// `background: transparent !important` on html/body (the selection surface
// must show the screen through the window), which also blanks the OCR pill's
// dark page: on macOS the WKWebView under a transparent page is white, and
// the pill's white label becomes invisible (v0.76.4 fix — the two components
// were co-imported in one branch, so the overlay's rule always won).
if (window.location.hash === '#ocr-progress') {
  const { default: OcrProgressIndicator } = await import(
    './lib/components/OcrProgressIndicator.svelte'
  );
  mount(OcrProgressIndicator, {
    target: document.getElementById('app')!,
  });
} else if (window.location.hash === '#screen-region-overlay') {
  // The X11/Windows capture overlay: a transparent fullscreen selection
  // surface. It must not import app.css or any store — it has to stay
  // transparent and featherweight, and the app's onboarding/recovery gates
  // must never run for a selection surface.
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
