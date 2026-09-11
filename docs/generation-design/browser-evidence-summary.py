"""Aggregate persisted evidence; no browser or production mutation."""
import json,pathlib,re,collections
root=pathlib.Path(__file__).resolve().parent
d=json.loads((root/'browser-results.json').read_text())
def luminance(rgb):
    values=[int(x)/255 for x in re.findall(r'\d+',rgb)[:3]]
    v=[x/12.92 if x<=0.04045 else ((x+0.055)/1.055)**2.4 for x in values]
    return sum(x*y for x,y in zip(v,[0.2126,0.7152,0.0722]))
colors=[]
for r in d['cases']:
    for c in r.get('badgeColors',[]):
        a,b=sorted([luminance(c['foreground']),luminance(c['background'])])
        colors.append({'theme':r['theme'],**c,'contrast':round((b+0.05)/(a+0.05),3)})
unique={json.dumps(c,sort_keys=True):c for c in colors}
summary={'counts':d.get('counts'),'unexpectedBoundaryCalls':sum(len(r.get('audit',{}).get('unknown',[])) for r in d['cases']),'blockedRequests':sum(len(r.get('blockedRequests',[])) for r in d['cases']),'browserErrors':sum(len(r.get('errors',[])) for r in d['cases']),'expectedSyntheticFailures':sum(len(r.get('audit',{}).get('expectedFailures',[])) for r in d['cases']),'geometrySamples':sum(sum(g['count'] for g in r.get('geometry',[])) for r in d['cases']),'badgeContrast':list(unique.values()),'productionDefects':d['errors'],'visuallyInspected':['empty-dark-390x360.png','active-light-390x360.png','output-completed-dark-390x360.png'],'visualFindings':'No visible overlaps or horizontal clipping in inspected images. Short-height output/context intentionally extend below viewport; scroll reachability tested separately.','limits':['No native App startup, DB, settings reads, patient data, OS clipboard, provider or external service.','SpeedRead dispatch sink mocked, reader overlay not mounted.','Only SOAP generation/Preview/Copy/SpeedRead and retry exercised, not all document-specific fields.','Freshness backend missing. Real GenerateTab remains Unknown after mocked successful generation; no current/stale verdict injected.','No complete accessibility claim or production lint/build rerun.'],'priorHarnessFailures':{'report':'browser-second-attempt.json','passed':48,'failed':12,'cause':'Active badge CSS uppercase makes innerText ACTIVE; semantic textContent Active. Corrected harness assertion only. Active child boundary calls discovered fail-closed and explicitly mocked after source inspection.','missingThemePairs':6,'identicalNonemptyHashPairs':0,'preservedScreenshots':'browser-evidence/initial-errors/'},'server':{'url':'http://127.0.0.1:14749/docs/generation-design/browser-index.html?state=completed&theme=dark','process':'proc_0a0c0c6b4c8e','ownership':'handed off to parent','restart':'../../node_modules/.bin/vite --config docs/generation-design/browser-vite.config.ts'},'rerun':'uv run --with playwright python docs/generation-design/browser-verify.py && python docs/generation-design/browser-evidence-summary.py'}
(root/'browser-summary.json').write_text(json.dumps(summary,indent=2));print(json.dumps(summary,indent=2))
