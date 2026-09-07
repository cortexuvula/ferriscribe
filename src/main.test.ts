import { readFileSync } from 'node:fs';
import { describe, expect, it } from 'vitest';

// The hash-routed special pages must stay style-isolated: main.ts must
// import ONLY the component the hash selects. ScreenRegionOverlay ships a
// page-global `background: transparent !important` on html/body (required
// for its see-through selection surface); when both components were
// co-imported in one branch that rule also applied to the OCR pill's page,
// blanking its dark background to the WKWebView's default white on macOS —
// an invisible pill (fixed v0.76.4). Component CSS is stubbed under vitest,
// so the import split is pinned at the source level instead.
const source = readFileSync(new URL('./main.ts', import.meta.url), 'utf8');

function routeBranch(marker: string): string {
  const after = source.slice(source.indexOf(marker));
  return after.slice(0, after.indexOf('else'));
}

describe('main.ts hash-route style isolation', () => {
  it('imports only OcrProgressIndicator on the #ocr-progress route', () => {
    const branch = routeBranch("'#ocr-progress'");
    expect(branch).toContain("'./lib/components/OcrProgressIndicator.svelte'");
    expect(branch).not.toContain('ScreenRegionOverlay');
  });

  it('imports only ScreenRegionOverlay on the #screen-region-overlay route', () => {
    const branch = routeBranch("'#screen-region-overlay'");
    expect(branch).toContain("'./lib/components/ScreenRegionOverlay.svelte'");
    expect(branch).not.toContain('OcrProgressIndicator');
  });
});
