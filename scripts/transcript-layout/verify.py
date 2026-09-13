"""Synthetic production-component layout gate; no native/patient resources.
Run: uv run --with playwright==1.58.0 python scripts/transcript-layout/verify.py
Uses CHROME_PATH if supplied, installed macOS Chrome, or Playwright Chromium.
"""
import argparse
import hashlib
import itertools
import json
import os
from pathlib import Path
import subprocess
import time
from urllib.parse import urlencode, urlparse
from urllib.request import urlopen

from playwright.sync_api import sync_playwright, expect

ROOT = Path(__file__).resolve().parents[2]
BASE = 'http://127.0.0.1:14873/scripts/transcript-layout/index.html'
REASONS = {
    'fold_possible': 'This transcript contains spans with no identified speaker, and the saved metadata shows unlabelled speech after a labelled span — speaker attribution cannot be verified.',
    'unknown': 'Speaker attribution cannot be verified from the saved metadata.',
}
REMEDY = 'Re-transcribe this recording to get an honest copy. Re-transcription may replace manual corrections you have made, and it produces a new attribution attempt — speaker labels are not guaranteed correct.'
GEOMETRY = """() => {
 const rect = r => ({left:r.left,top:r.top,right:r.right,bottom:r.bottom,width:r.width,height:r.height});
 const failures=[];
 const elements=[...document.querySelectorAll('.editor-header button,.btn-edit,.copy-block-message,.copy-block-reason,.copy-block-remedy')];
 const boxes=elements.map(e=>{
   const r=e.getBoundingClientRect();
   const label=e.textContent.trim();
   const clipped=(b,c)=>b.left<c.left-1||b.right>c.right+1||b.top<c.top-1||b.bottom>c.bottom+1;
   if(r.width<=0||r.height<=0||clipped(r,{left:0,top:0,right:innerWidth,bottom:innerHeight})) failures.push('outside viewport: '+label);
   for(let a=e.parentElement;a;a=a.parentElement){
     const s=getComputedStyle(a),ar=a.getBoundingClientRect();
     if(/hidden|auto|scroll|clip/.test(s.overflowX) && (r.left<ar.left-1||r.right>ar.right+1))failures.push('ancestor x clipping: '+label);
     if(/hidden|auto|scroll|clip/.test(s.overflowY) && (r.top<ar.top-1||r.bottom>ar.bottom+1))failures.push('ancestor y clipping: '+label);
   }
   const range=document.createRange();range.selectNodeContents(e);
   const lines=[...range.getClientRects()].map(rect);
   if(lines.some(b=>clipped(b,r)))failures.push('text clipping: '+label);
   if(e.tagName==='BUTTON'){
     const hit=document.elementFromPoint(r.left+r.width/2,r.top+r.height/2);
     if(!hit||!e.contains(hit))failures.push('not hit-testable: '+label);
   }
   return {label,rect:rect(r),lines};
 });
 const peers=elements.filter(e=>e.tagName==='BUTTON'||e.classList.contains('copy-block-message'));
 for(let i=0;i<peers.length;i++)for(let j=i+1;j<peers.length;j++){
   const a=peers[i].getBoundingClientRect(),b=peers[j].getBoundingClientRect();
   if(Math.min(a.right,b.right)-Math.max(a.left,b.left)>1 && Math.min(a.bottom,b.bottom)-Math.max(a.top,b.top)>1)failures.push('overlap: '+peers[i].textContent.trim()+' / '+peers[j].textContent.trim());
 }
 if(document.documentElement.scrollWidth>innerWidth)failures.push('document overflow');
 return {failures,boxes,theme:document.documentElement.dataset.theme,surface:getComputedStyle(document.body).backgroundColor,forced:matchMedia('(forced-colors: active)').matches,filter:getComputedStyle(document.documentElement).filter,viewport:{width:innerWidth,height:innerHeight},audit:window.audit};
}"""


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--out', default='node_modules/.cache/transcript-layout/evidence')
    parser.add_argument('--smoke', action='store_true')
    args = parser.parse_args()
    out = Path(args.out).resolve()
    out.mkdir(parents=True, exist_ok=True)
    result = {'commit': subprocess.check_output(['git', 'rev-parse', 'HEAD'], cwd=ROOT, text=True).strip(),
              'source_sha256': hashlib.sha256((ROOT/'src/lib/pages/EditorTab.svelte').read_bytes()).hexdigest(),
              'blocked': [], 'absence': [], 'positive': [], 'errors': [], 'external': []}
    def persist():
        (out/'results.json').write_text(json.dumps(result, indent=2))
    with (out/'vite.log').open('w') as log:
        server = subprocess.Popen([str(ROOT/'node_modules/.bin/vite'), '--config', 'scripts/transcript-layout/vite.config.ts'], cwd=ROOT, stdout=log, stderr=log)
        try:
            for _ in range(100):
                if server.poll() is not None:
                    raise RuntimeError('Vite exited; see vite.log')
                try:
                    with urlopen(BASE, timeout=1) as response:
                        if response.status == 200:
                            break
                except OSError:
                    time.sleep(.1)
            else:
                raise RuntimeError('Vite readiness failed')
            with sync_playwright() as p:
                chrome = os.environ.get('CHROME_PATH')
                mac_chrome = Path('/Applications/Google Chrome.app/Contents/MacOS/Google Chrome')
                if not chrome and mac_chrome.exists():
                    chrome = str(mac_chrome)
                browser = p.chromium.launch(headless=True, **({'executable_path': chrome} if chrome else {}))
                result['browser'] = browser.version
                ctx = browser.new_context(device_scale_factor=1)
                def route(r):
                    if urlparse(r.request.url).hostname == '127.0.0.1':
                        r.continue_()
                    else:
                        result['external'].append(r.request.url)
                        r.abort()
                ctx.route('**/*', route)
                page = ctx.new_page()
                page.on('pageerror', lambda e: result['errors'].append(str(e)))
                def visit(theme, mode, size, **query):
                    page.set_viewport_size(dict(zip(['width', 'height'], size)))
                    page.emulate_media(color_scheme=theme, forced_colors='active' if mode == 'forced' else 'none', reduced_motion='reduce')
                    page.goto(BASE+'?'+urlencode(dict(theme=theme, **query)))
                    page.wait_for_selector('.transcript-view')
                    page.evaluate('(mode)=>document.documentElement.style.filter=mode==="grayscale"?"grayscale(1)":"none"', mode)
                    expect(page.locator('html')).to_have_attribute('data-theme', theme)
                    assert page.evaluate("matchMedia('(forced-colors: active)').matches") == (mode == 'forced')
                    assert page.evaluate('getComputedStyle(document.documentElement).filter') == ('grayscale(1)' if mode == 'grayscale' else 'none')
                def capture(name):
                    d = page.evaluate(GEOMETRY)
                    screenshot = out/(name+'.png')
                    page.screenshot(path=str(screenshot), animations='disabled')
                    d['screenshot'] = screenshot.name
                    d['sha256'] = hashlib.sha256(screenshot.read_bytes()).hexdigest()
                    assert not d['audit']['unknown'], d['audit']
                    return d
                sizes = [(390,800),(360,640),(320,640),(800,800)]
                cases = list(itertools.product(['light','dark'], ['normal','grayscale','forced'], sizes, REASONS, ['absent','0','1']))
                if args.smoke:
                    cases = cases[:1]
                for theme, mode, size, evidence, version in cases:
                    visit(theme, mode, size, bypass='1', evidence=evidence, version=version)
                    page.get_by_role('button', name='Copy', exact=True).click()
                    expect(page.get_by_test_id('copy-block-message')).to_be_visible()
                    expect(page.locator('.copy-block-reason')).to_have_text(REASONS[evidence])
                    expect(page.locator('.copy-block-remedy')).to_have_text(REMEDY)
                    expect(page.get_by_role('button', name='Copy blocked', exact=True)).to_be_disabled()
                    page.mouse.move(0, 0)  # No hover needed for reason or remedy.
                    d = capture(f'blocked-{theme}-{mode}-{size[0]}-{evidence}-{version}')
                    d.update(mode=mode, evidence=evidence, version=version)
                    assert d['audit']['copied'] == []
                    assert len(page.locator('.btn-edit').all()) == 1
                    # Record original geometry BEFORE Playwright can scroll a target into view.
                    if not d['failures']:
                        if version == '0':
                            # Traverse from the user's Copy interaction, without
                            # programmatic focus or scrollIntoView assistance.
                            for _ in range(12):
                                page.keyboard.press('Tab')
                                if page.locator('.btn-edit').evaluate('(e)=>e===document.activeElement'):
                                    break
                            else:
                                raise AssertionError('Edit unreachable by Tab')
                            page.keyboard.press('Enter')
                            d['editMethod'] = 'Tab then Enter'
                        else:
                            page.locator('.btn-edit').click()
                            d['editMethod'] = 'pointer'
                        expect(page.locator('textarea')).to_be_visible()
                        d['editOpened'] = True
                    result['blocked'].append(d)
                    persist()
                if not args.smoke:
                    for theme, mode, outcome in itertools.product(['light','dark'], ['normal','grayscale','forced'], ['completed-with-unassigned','skipped','unknown']):
                        visit(theme, mode, (360,640), outcome=outcome)
                        expected = {'completed-with-unassigned':'Speaker unassigned', 'skipped':"Speaker labelling wasn't run", 'unknown':'Speaker-labelling status unavailable'}[outcome]
                        expect(page.locator('.transcript-view')).to_contain_text(expected)
                        d = capture(f'absence-{theme}-{mode}-{outcome}')
                        d.update(mode=mode, outcome=outcome)
                        result['absence'].append(d)
                        persist()
                    visit('light','normal',(320,640),bypass='1',version='2')
                    page.get_by_role('button',name='Copy',exact=True).click()
                    expect(page.get_by_role('button',name='Copied!',exact=True)).to_be_visible()
                    assert len(page.evaluate('window.audit.copied')) == 1
                    result['positive'].append({'version':2,'clipboardWrites':1})
                    # Both selected themes must paint, not merely change a query parameter.
                    for light in (r for r in result['blocked'] if r['theme']=='light'):
                        dark = next(r for r in result['blocked'] if r['theme']=='dark' and all(r[k]==light[k] for k in ['mode','viewport','evidence','version']))
                        assert dark['sha256'] != light['sha256']
                        assert dark['surface'] != light['surface']
                browser.close()
            result['counts'] = {key:len(result[key]) for key in ['blocked','absence','positive']}
            result['layoutFailures'] = sum(bool(d['failures']) for key in ['blocked','absence'] for d in result[key])
            persist()
            assert result['counts']['blocked'] == (1 if args.smoke else 144)
            assert args.smoke or result['counts']['absence'] == 18
            assert not result['errors'] and not result['external'], result
            assert result['layoutFailures'] == 0, f"{result['layoutFailures']} layout cases failed; see {out/'results.json'}"
            print(json.dumps({'counts':result['counts'],'layoutFailures':result['layoutFailures'],'browser':result['browser'],'out':str(out)}))
        finally:
            persist()
            server.terminate()
            server.wait(timeout=10)


if __name__ == '__main__':
    main()
