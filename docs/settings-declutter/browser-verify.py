"""Isolated production-component evidence; no real browser profile or backend."""
import json, re, hashlib
from pathlib import Path
from playwright.sync_api import sync_playwright, expect
OUT=Path(__file__).resolve().parent
URL='http://127.0.0.1:14739/docs/settings-declutter/browser-index.html'
results: dict={'renders':[], 'interactions':[], 'problemStates':[], 'errors':[], 'blockedRequests':[], 'contrast':[]}
GEOMETRY='''() => ({document:{client:document.documentElement.clientWidth,scroll:document.documentElement.scrollWidth}, panels:[...document.querySelectorAll('.modal-container,.modal-body,.settings-layout,.settings-nav,.settings-main,.settings-content,.settings-page,section,details,textarea,select')].filter(e=>e.getClientRects().length && e.clientWidth).map(e=>({element:e.tagName+'.'+e.className,width:e.clientWidth,scroll:e.scrollWidth,overflow:getComputedStyle(e).overflowX})).filter(e=>e.scroll>e.width+1)})'''
def persist(): (OUT/'browser-results.json').write_text(json.dumps(results,indent=2))
def luminance(c):
 vals=[float(v)/255 for v in re.findall(r'[\d.]+',c)[:3]]
 vals=[v/12.92 if v<=.04045 else ((v+.055)/1.055)**2.4 for v in vals]
 return sum(a*b for a,b in zip(vals,[.2126,.7152,.0722]))
with sync_playwright() as p:
 browser=p.chromium.launch(executable_path='/Applications/Google Chrome.app/Contents/MacOS/Google Chrome',headless=True)
 context=browser.new_context(viewport={'width':1200,'height':900})
 def route(r):
  if r.request.url.startswith('http://127.0.0.1:14739/'): r.continue_()
  else: results['blockedRequests'].append(r.request.url); r.abort()
 context.route('**/*',route)
 page=context.new_page()
 page.on('pageerror',lambda e:results['errors'].append({'url':page.url,'type':'pageerror','message':str(e)}))
 page.on('console',lambda m:results['errors'].append({'url':page.url,'type':m.type,'message':m.text}) if m.type=='error' else None)
 def load(section,theme='dark',case=''):
  page.goto(f'{URL}?page={section}&theme={theme}&case={case}')
  page.locator('.settings-page').wait_for()
  if section=='prompts': expect(page.locator('textarea')).to_have_value(re.compile('SYNTHETIC'))
  if section=='models': expect(page.locator('#ai-model')).to_be_enabled()
  page.wait_for_timeout(150)
 def audit(): return page.evaluate('window.fixtureAudit')
 for theme in ['light','dark']:
  for section in ['general','models','prompts']:
   for width in [390,800,1200]:
    page.set_viewport_size({'width':width,'height':900}); load(section,theme)
    geometry=page.evaluate(GEOMETRY)
    shot=f'browser-production-{section}-{theme}'+('' if width==1200 else f'-{width}')+'.png'
    page.screenshot(path=str(OUT/shot),full_page=True)
    results['renders'].append({'theme':theme,'page':section,'width':width,'screenshot':shot,'geometry':geometry,'audit':audit()})
    if width==1200:
     colors=page.locator('.nav-item').evaluate_all('''els=>els.map(e=>{let s=getComputedStyle(e); let bg=s.backgroundColor; let n=e;while(bg==='rgba(0, 0, 0, 0)' && n.parentElement){n=n.parentElement;bg=getComputedStyle(n).backgroundColor;}return {label:e.innerText,active:e.hasAttribute('aria-current'),fg:s.color,bg};})''')
     for c in colors:
      l1,l2=sorted([luminance(c['fg']),luminance(c['bg'])]);c['ratio']=(l2+.05)/(l1+.05);c['passes']=c['ratio']>=4.5
     results['contrast'].append({'theme':theme,'page':section,'colors':colors})
    persist()
 def test(name,fn):
  try: fn(); results['interactions'].append({'name':name,'pass':True})
  except Exception as e: results['interactions'].append({'name':name,'pass':False,'error':str(e)})
  persist()
 page.set_viewport_size({'width':1200,'height':900})
 def general():
  load('general'); check=page.get_by_label('Enable Autosave'); check.uncheck(); expect(page.locator('#autosave-interval')).to_be_disabled()
  assert page.evaluate('window.fixtureSettings.state.autosave_enabled') is False
  check.check(); page.locator('#autosave-interval').fill('9'); page.locator('#autosave-interval').press('Tab'); expect(page.get_by_role('alert')).to_contain_text('10 and 600')
  page.get_by_role('button',name='AI Models',exact=True).focus(); page.keyboard.press('Enter'); expect(page.locator('#ai-provider')).to_be_visible()
 test('General immediate simulated save, interval validation, keyboard sidebar navigation',general)
 def models():
  load('models'); summary=page.locator('summary').filter(has_text='Model options').first
  assert not summary.evaluate('e=>e.parentElement.open'); summary.focus(); page.keyboard.press('Enter'); expect(page.get_by_label('Translation Model',exact=True)).to_be_visible()
  page.get_by_label('Translation Model',exact=True).select_option('qwen3:1.7b'); assert page.evaluate('window.fixtureSettings.state.translation_model')=='qwen3:1.7b'
  summary.press('Space'); assert not summary.evaluate('e=>e.parentElement.open')
 test('Models options collapsed, keyboard disclosure, override save',models)
 def prompts():
  load('prompts'); select=page.get_by_label('Document type'); editor=page.locator('textarea'); editor.fill('SYNTHETIC UNSAVED DRAFT'); select.select_option('referral')
  expect(page.get_by_role('alertdialog')).to_be_visible(); page.keyboard.press('Escape'); expect(select).to_have_value('soap'); expect(editor).to_have_value('SYNTHETIC UNSAVED DRAFT'); expect(page.get_by_role('dialog',name='Settings',exact=True)).to_be_visible()
  select.select_option('referral'); page.get_by_role('button',name='Discard',exact=True).click(); expect(editor).to_have_value(re.compile('referral'))
  editor.fill('SYNTHETIC SAVE DRAFT'); before=len(audit()['calls']); page.get_by_role('button',name='Save as custom').click(); expect(page.get_by_text('custom',exact=True)).to_be_visible(); assert len(audit()['calls'])>before
  page.keyboard.press('Escape'); expect(page.get_by_role('dialog',name='Settings',exact=True)).to_have_count(0)
  page.get_by_role('button',name='Reopen fixture settings').click(); expect(select).to_be_visible()
 test('Prompt selector cancel/accept discard, explicit save, Escape close/reopen',prompts)
 def focus():
  load('general'); page.locator('.modal-close').focus(); page.keyboard.press('Shift+Tab'); assert page.evaluate("document.activeElement.closest('[role=dialog]')!==null")
  for _ in range(35):
   page.keyboard.press('Tab'); assert page.evaluate("document.activeElement.closest('[role=dialog]')!==null")
 test('Settings keyboard focus remains trapped for 35 tabs',focus)
 def tab_disclosures():
  for section,label in [('models','Model options'),('prompts','Help')]:
   load(section); page.locator('.modal-close').focus()
   target=page.locator('summary').filter(has_text=label).first
   for _ in range(45):
    if target.evaluate('e=>e===document.activeElement'): break
    page.keyboard.press('Tab')
   assert target.evaluate('e=>e===document.activeElement'), f'{label} unreachable by Tab'
   page.keyboard.press('Enter'); assert target.evaluate('e=>e.parentElement.open')
 test('Tab (not programmatic focus) reaches Model options and Prompts Help; Enter opens',tab_disclosures)
 for theme in ['light','dark']:
  for section,case in [('general','save-failure'),('models','offline'),('prompts','save-failure'),('about','restart-public')]:
   try:
    load(section,theme,case)
    details={}
    if section=='general':
     page.get_by_label('Enable Autosave').click(); page.wait_for_timeout(200)
     details={'storeRollback':page.evaluate('window.fixtureSettings.state.autosave_enabled') is True,'controlRollback':page.get_by_label('Enable Autosave').is_checked()}
    if section=='prompts':
     page.locator('textarea').fill('SYNTHETIC UNSAVED FAILURE DRAFT');page.get_by_role('button',name='Save as custom').click();page.wait_for_timeout(200)
     details={'draftRetained':page.locator('textarea').input_value()=='SYNTHETIC UNSAVED FAILURE DRAFT','savedValue':page.evaluate('window.fixtureSettings.state.custom_soap_prompt')}
    text=page.locator('body').inner_text(); details['visibleFeedback']=('SYNTHETIC FIXTURE save failed' in text or 'Could not save' in text or 'Failed to save' in text) if case=='save-failure' else ('provider offline' in text if case=='offline' else 'Restart' in text and 'Public endpoints enabled' in text)
    shot=f'browser-problem-{section}-{theme}.png'; page.screenshot(path=str(OUT/shot),full_page=True)
    results['problemStates'].append({'theme':theme,'page':section,'case':case,'details':details,'text':text,'screenshot':shot,'geometry':page.evaluate(GEOMETRY),'audit':audit()})
   except Exception as e:results['problemStates'].append({'theme':theme,'page':section,'case':case,'error':str(e)})
   persist()
 browser.close()
# Fingerprints identify the source observed, without modifying or committing it.
root=OUT.parent.parent
results['sourceHashes']={str(f.relative_to(root)):hashlib.sha256(f.read_bytes()).hexdigest() for f in [root/'src/lib/components/SettingsContent.svelte',root/'src/lib/components/Modal.svelte',root/'src/lib/components/settings/Models.svelte',root/'src/lib/components/settings/Prompts.svelte',root/'src/lib/components/settings/sections/GeneralBasics.svelte',root/'src/lib/components/settings/settings.css']}
persist()
print(json.dumps({'renders':len(results['renders']),'interactions':results['interactions'],'problems':[{k:v for k,v in x.items() if k in ['theme','page','details','error']} for x in results['problemStates']],'overflowCases':sum(bool(x['geometry']['panels']) for x in results['renders']),'unknownCalls':[x['audit']['unknown'] for x in results['renders'] if x['audit']['unknown']],'contrastMinimum':min(c['ratio'] for x in results['contrast'] for c in x['colors']),'errors':results['errors'],'blockedRequests':results['blockedRequests']},indent=2))
