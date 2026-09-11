from playwright.sync_api import sync_playwright
with sync_playwright() as p:
 b=p.chromium.launch(executable_path='/Applications/Google Chrome.app/Contents/MacOS/Google Chrome',headless=True)
 page=b.new_page()
 page.on('pageerror',lambda e:print('PAGEERROR',e))
 page.on('console',lambda m:print('CONSOLE',m.type,m.text) if m.type=='error' else None)
 page.on('response',lambda r:print('FAILED RESPONSE',r.status,r.url,r.text()[:4000]) if r.status>=400 else None)
 page.goto('http://127.0.0.1:14739/docs/settings-declutter/browser-index.html')
 page.wait_for_timeout(2500)
 print(page.locator('body').inner_text())
 b.close()
