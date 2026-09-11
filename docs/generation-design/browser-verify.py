"""Real GenerateTab render gate. Synthetic boundary data; not backend freshness integration."""
import hashlib, json, pathlib, sys, itertools
from playwright.sync_api import sync_playwright
ROOT=pathlib.Path(__file__).resolve().parent
OUT=ROOT/'browser-evidence'; OUT.mkdir(exist_ok=True)
RESULT=ROOT/'browser-results.json'
results={'scope':'Real GenerateTab and stores; mocked Tauri, clipboard, SpeedRead sink; no App startup or provenance backend','cases':[],'pairs':[],'errors':[],'interactions':[]}
def save(): RESULT.write_text(json.dumps(results,indent=2))
def require(value,message):
    if not value: raise AssertionError(message)
def distinct(a,b): require(a!=b,'Identical paired theme hashes')
try:
    distinct('same','same')
    raise RuntimeError('Hash guard did not reject identical artifacts')
except AssertionError: results['identical_hash_guard_test']='passed'
with sync_playwright() as p:
    browser=p.chromium.launch(executable_path='/Applications/Google Chrome.app/Contents/MacOS/Google Chrome',headless=True)
    for state,theme,width,height in itertools.product(['empty','active','generating','failed','completed'],['light','dark'],[390,800,1200],[700,360]):
        row={'state':state,'theme':theme,'width':width,'height':height,'errors':[]}
        ctx=browser.new_context(viewport={'width':width,'height':height},reduced_motion='reduce')
        page=ctx.new_page(); page.set_default_timeout(4500)
        page.on('pageerror',lambda e:row['errors'].append(str(e)))
        blocked=[]
        def route(r):
            if r.request.url.startswith('http://127.0.0.1:14749/') or r.request.url.startswith('data:'): r.continue_()
            else: blocked.append(r.request.url);r.abort()
        ctx.route('**/*',route)
        try:
            page.goto(f'http://127.0.0.1:14749/docs/generation-design/browser-index.html?state={state}&theme={theme}',wait_until='networkidle')
            require(page.locator('.generate-tab').count()==1,'Real tab missing/duplicated')
            require(page.get_by_role('heading',name='Generate Documentation',exact=True).count()==1,'Wrong main heading')
            if state=='empty':
                require(page.locator('.empty-state').count()==1 and page.locator('.generate-content').count()==0,'Wrong empty state')
                page.get_by_role('button',name='Go to Recordings',exact=True).click()
                require(page.evaluate('fixture.audit.navigation')==1,'Empty recovery callback missing')
            else:
                require(page.locator('.generate-content').count()==1 and page.locator('.empty-state').count()==0,'Wrong populated state')
                require(page.get_by_role('heading',name='Create document',exact=True).count()==1,'Create heading missing')
                require(page.get_by_role('heading',name='Recent output',exact=True).count()==1,'Output heading missing')
                if state=='active':
                    require(page.get_by_label('Notes',exact=True).input_value()=='SYNTHETIC supporting notes','Active notes not restored')
                    require(page.locator('.context-badge').text_content().strip()=='Active','Active badge missing')
                if state in ['generating','failed','completed']:
                    page.evaluate('(mode)=>fixture.setMode(mode)',{'generating':'pending','failed':'failure','completed':'success'}[state])
                    page.get_by_role('button',name='Generate SOAP note',exact=True).click()
                    if state=='generating':
                        page.get_by_role('status').wait_for();require(page.get_by_role('button',name='Generating SOAP note…',exact=True).is_disabled(),'Generate not locked')
                    elif state=='failed':
                        page.get_by_role('alert').wait_for();require('SYNTHETIC generation failed' in page.get_by_role('alert').inner_text(),'Failure absent')
                        require(page.locator('.failed-badge').inner_text().strip()=='Failed','Failed item absent')
                    else:
                        page.locator('.output-badge').wait_for();require(page.locator('.output-badge').inner_text().strip()=='Freshness unavailable','Success falsely marked current')
                        require(page.locator('.output-hint').count()==1,'Unknown provenance explanation absent')
            row['badgeColors']=page.locator('.output-badge').evaluate_all('es=>es.map(e=>({foreground:getComputedStyle(e).color,background:getComputedStyle(e).backgroundColor}))')
            row['computed']=page.evaluate('''() => ({theme:document.documentElement.dataset.theme,bg:getComputedStyle(document.body).backgroundColor,fg:getComputedStyle(document.body).color})''')
            require(row['computed']['theme']==theme,'Theme attribute mismatch')
            require(row['computed']['bg']=={'light':'rgb(255, 255, 255)','dark':'rgb(26, 27, 30)'}[theme],'Computed theme mismatch')
            row['geometry']=page.evaluate('''() => {const sels=['.generate-tab', ...(document.querySelector('.generate-content')?['.generate-content','.context-panel','.create-section','.output-section']:['.empty-state'])];return sels.map(selector=>{const es=[...document.querySelectorAll(selector)];return {selector,count:es.length,elements:es.map(e=>{const r=e.getBoundingClientRect();return {left:r.left,right:r.right,top:r.top,bottom:r.bottom,width:r.width,height:r.height,scrollWidth:e.scrollWidth,clientWidth:e.clientWidth,scrollHeight:e.scrollHeight,clientHeight:e.clientHeight}})}})}''')
            for sample in row['geometry']:
                require(sample['count']>0,'Empty geometry selector '+sample['selector'])
                for e in sample['elements']:
                    require(e['width']>0 and e['height']>0,'Zero-size panel')
                    require(e['left']>=-1 and e['right']<=width+1 and e['scrollWidth']<=e['clientWidth']+1,'Inner horizontal overflow '+sample['selector'])
                    if sample['selector'] in ['.generate-tab','.generate-content','.empty-state']:
                        require(e['top']>=-1 and e['bottom']<=height+1,'Scroll region exceeds viewport '+sample['selector'])
            require(page.evaluate('document.documentElement.scrollWidth<=innerWidth && document.documentElement.scrollHeight<=innerHeight'),'Document overflow')
            actions=page.locator('.generate-tab button:visible, .generate-tab summary:visible, .generate-tab textarea:visible, .generate-tab input:visible, .generate-tab select:visible')
            row['reachability']=[];require(actions.count()>0,'No action samples')
            for i in range(actions.count()):
                a=actions.nth(i); a.scroll_into_view_if_needed()
                m=a.evaluate('''e=>{const r=e.getBoundingClientRect(),x=r.x+r.width/2,y=r.y+r.height/2;const hit=document.elementFromPoint(x,y);return {text:(e.innerText||e.id).trim(),height:r.height,x,y,hit:!!hit&&(e===hit||e.contains(hit)),disabled:!!e.disabled}}''')
                row['reachability'].append(m);require(m['hit'] and 0<=m['y']<height,'Unreachable action '+m['text'])
            if state!='empty': page.locator('.generate-content').evaluate('e=>e.scrollTop=0')
            name=f'{state}-{theme}-{width}x{height}.png'; path=OUT/name
            page.screenshot(path=str(path),animations='disabled');row['screenshot']=str(path);row['sha256']=hashlib.sha256(path.read_bytes()).hexdigest()
            if state=='completed':
                item=page.get_by_role('article',name='SOAP note',exact=True)
                summary=item.locator('summary');item.get_by_role('button',name='Regenerate',exact=True).focus();page.keyboard.press('Tab');require(summary.evaluate('e=>e===document.activeElement'),'Native Tab did not reach Preview');page.keyboard.press('Enter')
                require(item.locator('details').get_attribute('open') is not None,'Keyboard Preview failed')
                require('SYNTHETIC generated soap_note output' in item.locator('pre').inner_text(),'Preview wrong output')
                item.get_by_role('button',name='Copy',exact=True).click()
                require(page.evaluate('fixture.audit.copies[0]')==item.locator('pre').inner_text(),'Clipboard mock got wrong text')
                item.get_by_role('button',name='Speed Read',exact=True).click()
                require(page.evaluate('fixture.audit.speed[0].kind')=='soap','SpeedRead wrong dispatch')
                item.scroll_into_view_if_needed();page.screenshot(path=str(OUT/f'output-{name}'),animations='disabled')
                row['outputScreenshot']=str(OUT/f'output-{name}')
                row['actionsVerified']=['Generate through real API/store + mocked invoke','Preview keyboard Enter','Copy mock exact output','SpeedRead mock soap dispatch','Unknown after successful generation']
            elif state=='failed':
                page.evaluate("fixture.setMode('success')")
                page.get_by_role('alert').get_by_role('button',name='Retry',exact=True).click()
                page.locator('.output-badge').wait_for();require(page.locator('.output-badge').inner_text().strip()=='Freshness unavailable','Retry falsely current')
                row['actionsVerified']=['Generate rejected at mock boundary','Visible failure + Retry','Retry completion remains Unknown']
            elif state=='active':
                page.get_by_role('button',name='Additional Context',exact=False).click()
                require(page.locator('.context-summary').inner_text().strip()=='Included: medications, notes','Collapsed active summary wrong')
                row['actionsVerified']=['Saved context restored','Collapsed active context summary']
            row['audit']=page.evaluate('fixture.audit');row['blockedRequests']=blocked
            require(not row['audit']['unknown'],'Unknown boundary calls: '+str(row['audit']['unknown']))
            require(not blocked,'External request attempted')
            require(not row['errors'],'Browser page errors')
            row['passed']=True
        except Exception as e:
            row['passed']=False;row['errors'].append(str(e));results['errors'].append({'case':[state,theme,width,height],'error':str(e)})
            try:
                row['audit']=page.evaluate('window.fixture?.audit');page.screenshot(path=str(OUT/f'ERROR-{state}-{theme}-{width}x{height}.png'))
            except Exception: pass
        results['cases'].append(row);save();ctx.close()
    browser.close()
for state,width,height in itertools.product(['empty','active','generating','failed','completed'],[390,800,1200],[700,360]):
    pair=[r for r in results['cases'] if (r['state'],r['width'],r['height'])==(state,width,height)]
    valid=len(pair)==2 and all('sha256' in r for r in pair) and pair[0]['sha256']!=pair[1]['sha256']
    results['pairs'].append({'state':state,'width':width,'height':height,'distinct':valid})
results['counts']={'requested':60,'collected':len(results['cases']),'passed':sum(r['passed'] for r in results['cases']),'failed':sum(not r['passed'] for r in results['cases']),'themePairs':len(results['pairs']),'distinctPairs':sum(r['distinct'] for r in results['pairs']),'actionSamples':sum(len(r.get('reachability',[])) for r in results['cases'])}
save();print(json.dumps(results['counts']));print('Evidence:',RESULT)
sys.exit(0 if results['counts']['passed']==60 and results['counts']['distinctPairs']==30 else 1)
