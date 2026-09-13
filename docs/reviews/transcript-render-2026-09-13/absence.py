from pathlib import Path
import json
from playwright.sync_api import sync_playwright
P=Path(__file__).parent
rows=[]
with sync_playwright() as p:
 b=p.chromium.launch(executable_path='/Applications/Google Chrome.app/Contents/MacOS/Google Chrome',headless=True)
 page=b.new_page(viewport={'width':360,'height':640});page.goto('http://127.0.0.1:18743');page.wait_for_function('window.review')
 for theme in ['light','dark']:
  for mode in ['normal','grayscale','forced']:
   page.emulate_media(forced_colors='active' if mode=='forced' else 'none')
   page.evaluate('(t)=>document.documentElement.dataset.theme=t',theme)
   page.evaluate('(m)=>document.documentElement.style.filter=m==="grayscale"?"grayscale(1)":"none"',mode)
   for outcome in ['completed-with-unassigned','skipped','unknown']:
    page.evaluate('(o)=>window.review.set("Synthetic transcript body.",[{speaker:null,text:"Synthetic transcript body.",start:0,end:1}],o)',outcome)
    page.wait_for_timeout(450)
    file=P/f'absence-{theme}-{mode}-{outcome}.png';page.screenshot(path=str(file))
    rows.append({'theme':theme,'mode':mode,'outcome':outcome,'text':page.locator('#app').inner_text(),'overflow':page.evaluate('document.documentElement.scrollWidth > innerWidth'),'screenshot':str(file)})
 b.close()
(P/'absence-results.json').write_text(json.dumps(rows,indent=2));print(json.dumps({'cases':len(rows),'overflowCases':[r for r in rows if r['overflow']],'distinctTextStates':len(set(r['text'] for r in rows))},indent=2))
