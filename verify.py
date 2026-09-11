"""Render synthetic Generation states; fail closed on theme/state drift."""
from pathlib import Path
import hashlib
import json
import sys
from playwright.sync_api import sync_playwright

ROOT = Path(__file__).resolve().parent
STATES = ['empty', 'active', 'generating', 'failed', 'completed', 'stale']
THEMES = ['light', 'dark']
WIDTHS = [390, 800, 1200]


def check_distinct(root=ROOT):
    pairs = []
    for state in STATES:
        hashes = {theme: hashlib.sha256((root / f'generation-{state}-{theme}.png').read_bytes()).hexdigest() for theme in THEMES}
        assert hashes['light'] != hashes['dark'], f'{state}: light/dark screenshots are byte-identical'
        pairs.append({'state': state, **hashes})
    assert len(pairs) == len(STATES)
    return pairs


def luminance(rgb):
    values = [v / 255 for v in rgb]
    linear = [v / 12.92 if v <= 0.04045 else ((v + 0.055) / 1.055) ** 2.4 for v in values]
    return sum(v * w for v, w in zip(linear, [0.2126, 0.7152, 0.0722]))


def contrast(a, b):
    lo, hi = sorted([luminance(a), luminance(b)])
    return (hi + 0.05) / (lo + 0.05)


def render():
    report: dict = {'checks': [], 'stale_contrast': []}
    with sync_playwright() as p:
        browser = p.chromium.launch(executable_path='/Applications/Google Chrome.app/Contents/MacOS/Google Chrome', headless=True)
        page = browser.new_page()
        for theme in THEMES:
            for state in STATES:
                for width in WIDTHS:
                    page.set_viewport_size({'width': width, 'height': 900})
                    page.goto(ROOT.joinpath('generation-workspace.html').as_uri() + f'?theme={theme}#{state}')
                    page.wait_for_selector(f'#{state}')
                    page.wait_for_function('(theme) => document.documentElement.dataset.theme === theme', arg=theme, timeout=2000)
                    surface = page.evaluate('getComputedStyle(document.body).backgroundColor')
                    assert surface == {'light': 'rgb(255, 255, 255)', 'dark': 'rgb(26, 27, 30)'}[theme], (theme, surface)
                    visible = page.locator('.workspace > section:visible').evaluate_all('(nodes) => nodes.map(n => n.id)')
                    assert visible == [state], (theme, state, visible)
                    assert page.evaluate('document.documentElement.scrollWidth <= innerWidth'), (theme, state, width)
                    report['checks'].append({'theme': theme, 'state': state, 'width': width, 'surface': surface, 'overflow': False})
                    if state == 'stale':
                        # Composite transparent badge/ancestor backgrounds from body upwards.
                        colors = page.locator('#stale .badge').evaluate('''el => {
                            const rgba = s => s.match(/[\\d.]+/g).map(Number);
                            const chain = []; for (let n=el; n; n=n.parentElement) chain.unshift(n);
                            let bg = [255,255,255];
                            for (const n of chain) {
                                const c = rgba(getComputedStyle(n).backgroundColor), a = c[3] ?? 1;
                                bg = bg.map((v,i) => c[i]*a + v*(1-a));
                            }
                            const fg = rgba(getComputedStyle(el).color), a = fg[3] ?? 1;
                            return {foreground: bg.map((v,i) => fg[i]*a+v*(1-a)), background: bg,
                                text: el.textContent.trim(), fontSize: getComputedStyle(el).fontSize};
                        }''')
                        ratio = contrast(colors['foreground'], colors['background'])
                        assert colors['text'] == 'Stale'
                        assert ratio >= 4.5, (theme, width, ratio, colors)
                        report['stale_contrast'].append({'theme': theme, 'width': width, 'ratio': ratio, **colors})
                page.screenshot(path=str(ROOT / f'generation-{state}-{theme}.png'), full_page=True)
        browser.close()
    report['sha256_pairs'] = check_distinct()
    report['viewport_count'] = len(report['checks'])
    report['screenshot_count'] = len(report['sha256_pairs']) * len(THEMES)
    assert report['viewport_count'] == len(STATES) * len(THEMES) * len(WIDTHS)
    (ROOT / 'verification.json').write_text(json.dumps(report, indent=2) + '\n')
    print(json.dumps({'viewport_checks': report['viewport_count'], 'screenshots': report['screenshot_count'], 'distinct_pairs': len(report['sha256_pairs']), 'stale_contrast': report['stale_contrast']}, indent=2))


if __name__ == '__main__':
    if '--check-existing' in sys.argv:
        print(json.dumps(check_distinct(), indent=2))
    else:
        render()
