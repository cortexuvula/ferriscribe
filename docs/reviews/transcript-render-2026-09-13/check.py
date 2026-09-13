import json, subprocess, hashlib
from pathlib import Path
from playwright.sync_api import sync_playwright
P=Path(__file__).parent
rows=[]
with sync_playwright() as p:
    browser=p.chromium.launch(executable_path='/Applications/Google Chrome.app/Contents/MacOS/Google Chrome',headless=True)
    page=browser.new_page(viewport={'width':1000,'height':720})
    page.goto('http://127.0.0.1:18743')
    page.wait_for_function('window.review !== undefined')
    def setcase(value,segments,outcome,reset=True):
        page.evaluate('(a)=>window.review.set(...a)',[value,segments,outcome,reset])
        page.wait_for_timeout(450)
    def snap(name):
        row={'case':name,'text':page.locator('#app').inner_text(),'buttons':page.locator('#app button').all_text_contents(),'unassigned':page.locator('.unlabeled-heading').count(),'badges':page.locator('.speaker-badge').all_text_contents()}
        rows.append(row);return row
    for outcome in ['off','skipped','unknown']:
        setcase('First synthetic paragraph.\n\nSecond synthetic paragraph.',None,outcome)
        snap('two-paragraph-'+outcome)
    setcase('Single synthetic paragraph.',None,'unknown');snap('unknown-edit-access')
    segs=[{'speaker':'Speaker 1','text':'Alpha','start':0,'end':1},{'speaker':None,'text':'Gap','start':1,'end':2},{'speaker':'Speaker 2','text':'Beta','start':2,'end':3}]
    stored=subprocess.check_output([str(P/'formatter')],text=True).strip()
    setcase(stored,segs,'completed-with-unassigned');snap('formatter-with-metadata')
    setcase(stored,None,'completed-with-unassigned');snap('formatter-without-metadata')
    setcase('Speaker 1: ORIGINAL', [{'speaker':'Speaker 1','text':'ORIGINAL','start':0,'end':1}],'completed')
    page.get_by_role('button',name='Edit',exact=True).click()
    page.locator('textarea').fill('Speaker 1: CORRECTED')
    rows.append({'case':'before-done','parentValue':page.evaluate('window.review.get().value'),'draft':page.locator('textarea').input_value(),'changeCount':page.evaluate('window.review.changes.length')})
    page.get_by_role('button',name='Done',exact=True).click();page.wait_for_timeout(450)
    row=snap('after-done-stale-segments');row['parentValue']=page.evaluate('window.review.get().value')
    setcase('Speaker 1: CORRECTED',[{'speaker':'Speaker 1','text':'ORIGINAL','start':0,'end':1}],'completed');snap('reopened-stale-segments')
    setcase('Speaker 1: Record A',None,'completed')
    page.get_by_role('button',name='Edit',exact=True).click();page.locator('textarea').fill('Speaker 1: Record A draft')
    setcase('Speaker 2: Record B',None,'completed',False)
    row=snap('record-switch-while-editing');row['draft']=page.locator('textarea').input_value();row['parentValue']=page.evaluate('window.review.get().value')
    page.get_by_role('button',name='Done',exact=True).click();page.wait_for_timeout(450)
    row=snap('done-after-record-switch');row['parentValue']=page.evaluate('window.review.get().value')
    setcase('Same synthetic body.',[{'speaker':None,'text':'Same synthetic body.','start':0,'end':1}],'failed');failed=snap('failed')
    setcase('Same synthetic body.',[{'speaker':None,'text':'Same synthetic body.','start':0,'end':1}],'completed-with-unassigned');completed=snap('completed-unassigned')
    rows.append({'case':'failure-distinction','identicalVisibleText':failed['text']==completed['text']})
    # Representative production-component render, all palette entries plus an unassigned run.
    allsegs=[{'speaker':f'Speaker {i+1}','text':f'Synthetic attributed statement {i+1}.','start':i,'end':i+1} for i in range(8)]
    allsegs.insert(1,{'speaker':None,'text':'Synthetic unattributed statement. Do not infer who spoke.','start':1,'end':2})
    for theme in ['light','dark']:
      for mode in ['normal','grayscale','forced']:
        page.emulate_media(forced_colors='active' if mode=='forced' else 'none')
        page.evaluate('(t)=>document.documentElement.dataset.theme=t',theme)
        page.evaluate('(m)=>document.documentElement.style.filter=m==="grayscale"?"grayscale(1)":"none"',mode)
        setcase('synthetic text',allsegs,'completed-with-unassigned')
        colors=page.locator('.speaker-badge').evaluate_all('(els)=>els.map(e=>{let s=getComputedStyle(e);return {text:e.textContent.trim(),fg:s.color,bg:s.backgroundColor,fontSize:s.fontSize}})')
        rail=page.locator('.unlabeled').evaluate('(e)=>{let s=getComputedStyle(e);return {color:s.borderLeftColor,style:s.borderLeftStyle,bg:getComputedStyle(document.body).backgroundColor}}')
        img=P/f'{theme}-{mode}.png';page.screenshot(path=str(img))
        rows.append({'case':theme+'-'+mode,'badges':colors,'rail':rail,'screenshot':str(img),'sha256':hashlib.sha256(img.read_bytes()).hexdigest(),'forced':page.evaluate('matchMedia("(forced-colors: active)").matches')})
    browser.close()
(P/'results.json').write_text(json.dumps(rows,indent=2))
print(json.dumps(rows,indent=2))
