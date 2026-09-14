"""#108 contrast re-verification: computed styles in real Chromium.

Serves the WORKTREE (production TranscriptView + app.css, unmodified) with a
minimal Vite config, mounts 8 badges + unassigned section in both themes,
reads computed styles, alpha-composites over painted surfaces, and evaluates
WCAG luminance. Fails (exit 1) if any ratio < 4.5.
Usage: python3 issue108-contrast-verify.py <worktree-root>
"""
import json
import subprocess
import sys
import time
from pathlib import Path
from urllib.request import urlopen

from playwright.sync_api import sync_playwright

ROOT = Path(sys.argv[1]).resolve()
PORT = 14874

HTML = """<!doctype html><html><head><meta charset="utf-8">
<script type="module">
  import '/src/app.css';
  import { mount } from 'svelte';
  import TranscriptView from '/src/lib/components/TranscriptView.svelte';
  const params = new URLSearchParams(location.search);
  document.documentElement.dataset.theme = params.get('theme');
  const outcome = params.get('outcome');
  const segs = outcome === 'labels'
    ? [1,2,3,4,5,6,7,8].map(n => ({ speaker: 'Speaker '+n, text: 'Turn '+n+'.', start: 0, end: 1 }))
    : [{ speaker: null, text: 'Unattributed passage.', start: 0, end: 1 }];
  mount(TranscriptView, { target: document.getElementById('app'),
    props: { value: 'Synthetic.', segments: segs,
             diarizationOutcome: outcome === 'labels' ? 'completed' : outcome } });
</script>
</head><body><div id="app"></div></body></html>"""

VITE_CONFIG = f"""import {{ defineConfig }} from 'vite';
import {{ svelte }} from '@sveltejs/vite-plugin-svelte';
export default defineConfig({{
  plugins: [svelte()],
  server: {{ host: '127.0.0.1', port: {PORT}, strictPort: true }},
}});
"""

MEASURE = """(() => {
  const lum = rgb => {
    const ch = rgb.map(c => { c/=255; return c<=0.04045 ? c/12.92 : Math.pow((c+0.055)/1.055,2.4); });
    return 0.2126*ch[0]+0.7152*ch[1]+0.0722*ch[2];
  };
  const parse = s => s.replace(/^(rgba|rgb)\\(/,'').replace(/\\)$/,'').split(',').map(Number);
  const comp = (fg,bg) => fg.slice(0,3).map((c,i)=>fg[3]*c+(1-fg[3])*bg[i]);
  const ratio = (a,b) => { const [x,y]=[lum(a),lum(b)].sort((p,q)=>q-p); return (x+0.05)/(y+0.05); };
  const out = [];
  const surfaces = [parse(getComputedStyle(document.body).backgroundColor)];
  for (const badge of document.querySelectorAll('.speaker-badge')) {
    const s = getComputedStyle(badge);
    const text = parse(s.color), bg = parse(s.backgroundColor);
    const r = Math.min(...surfaces.map(surf => ratio(text, comp(bg, surf))));
    out.push({ kind: 'badge', label: badge.textContent.trim(), color: s.color,
               ratio: Math.round(r*100)/100 });
  }
  const h = document.querySelector('.unlabeled-heading');
  if (h) {
    const s = getComputedStyle(h);
    const r = Math.min(...surfaces.map(surf => ratio(parse(s.color), surf)));
    out.push({ kind: 'unassigned-heading', color: s.color, fontSize: s.fontSize,
               ratio: Math.round(r*100)/100 });
  }
  const rail = document.querySelector('.speaker-section.unlabeled');
  if (rail) {
    const s = getComputedStyle(rail);
    const r = Math.min(...surfaces.map(surf => ratio(parse(s.borderLeftColor), surf)));
    out.push({ kind: 'unassigned-rail', color: s.borderLeftColor,
               width: s.borderLeftWidth, style: s.borderLeftStyle,
               ratio: Math.round(r*100)/100 });
  }
  return out;
})()"""

def main():
    html_path = ROOT / 'issue108-contrast-verify.html'
    cfg_path = ROOT / 'issue108-contrast-vite.config.ts'
    html_path.write_text(HTML)
    cfg_path.write_text(VITE_CONFIG)
    try:
        vite = subprocess.Popen(
            [str(ROOT / 'node_modules/.bin/vite'),
             '--config', 'issue108-contrast-vite.config.ts'],
            cwd=ROOT, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        base = f'http://127.0.0.1:{PORT}/issue108-contrast-verify.html'
        for _ in range(100):
            try:
                with urlopen(base, timeout=1) as r:
                    if r.status == 200:
                        break
            except OSError:
                time.sleep(0.1)
        else:
            raise RuntimeError('vite readiness failed')
        rows, failures = [], []
        with sync_playwright() as p:
            chrome = Path('/Applications/Google Chrome.app/Contents/MacOS/Google Chrome')
            browser = p.chromium.launch(
                headless=True, **({'executable_path': str(chrome)} if chrome.exists() else {}))
            page = browser.new_page()
            for theme in ('light', 'dark'):
                for outcome in ('labels', 'completed-with-unassigned'):
                    page.goto(f'{base}?theme={theme}&outcome={outcome}')
                    page.wait_for_selector('.transcript-view')
                    page.wait_for_selector(
                        '.speaker-badge' if outcome == 'labels' else '.unlabeled-heading')
                    for row in page.evaluate(MEASURE):
                        row['theme'] = theme
                        row['outcome'] = outcome
                        rows.append(row)
                        if row['ratio'] < 4.5:
                            failures.append(row)
            browser.close()
        print(json.dumps(rows, indent=1))
        print(f'\n{len(rows)} measurements, {len(failures)} below 4.5:1')
        if failures:
            print('FAILURES:', json.dumps(failures, indent=1))
            sys.exit(1)
        print('ALL PASS')
    finally:
        vite.terminate()
        vite.wait(timeout=10)
        html_path.unlink(missing_ok=True)
        cfg_path.unlink(missing_ok=True)

if __name__ == '__main__':
    main()
