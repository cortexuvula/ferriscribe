from pathlib import Path
import json
from playwright.sync_api import sync_playwright
root=Path(__file__).parent
results=[]
with sync_playwright() as p:
    browser=p.chromium.launch(executable_path='/Applications/Google Chrome.app/Contents/MacOS/Google Chrome',headless=True)
    page=browser.new_page()
    for theme in ['light','dark']:
        for section in ['general','models','prompts']:
            for width in [390,800,1200]:
                page.set_viewport_size({'width':width,'height':900})
                page.goto(root.joinpath('proposal.html').as_uri()+f'?theme={theme}#{section}')
                page.wait_for_selector('main h1')
                overflow=page.evaluate('document.documentElement.scrollWidth>innerWidth')
                assert not overflow,(theme,section,width)
                if width==1200:
                    page.screenshot(path=str(root/f'{section}-{theme}.png'),full_page=True)
                results.append({'theme':theme,'page':section,'width':width,'overflow':overflow})
    page.goto(root.joinpath('proposal.html').as_uri()+'#models')
    page.locator('summary').first.click()
    assert page.locator('#host').is_visible()
    browser.close()
root.joinpath('proposal-checks.json').write_text(json.dumps(results,indent=2))
print(json.dumps({'renders':6,'viewport_checks':len(results),'horizontal_overflow':any(r['overflow'] for r in results),'disclosure_opens':True}))
