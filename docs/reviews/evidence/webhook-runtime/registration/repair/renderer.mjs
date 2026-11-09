import { chromium } from '/tmp/nessa-docs/node_modules/playwright/index.mjs';
import { writeFileSync } from 'node:fs';
const browser=await chromium.launch({executablePath:'/usr/bin/chromium',headless:true});
const page=await browser.newPage({viewport:{width:1500,height:1000}});
const errors=[];page.on('pageerror',e=>errors.push(String(e)));
const results=[];
for(const [id,count] of [['gateway-webhook-issuance',2],['gateway-webhook-outbox',2]]){
 await page.goto('http://127.0.0.1:4312/docs/'+id);
 await page.waitForSelector(`.document-layout[data-doc-id="${id}"]`);
 await page.waitForFunction(n=>document.querySelectorAll('figure.diagram').length===n && [...document.querySelectorAll('figure.diagram')].every(f=>f.dataset.state!=='rendering'),count);
 await page.evaluate(()=>document.fonts.ready);
 const served=await(await page.request.get('http://127.0.0.1:4312/api/documents/'+id)).json();
 if(served.document.sourceRevision!=='6cfb9673aba7c9fb78e26f4e3c09f2dd55be279f')throw new Error('wrong source checkpoint');
 for(let i=0;i<count;i++){
  const figure=page.locator('figure.diagram').nth(i);
  await figure.getByRole('button',{name:'Fit',exact:true}).click();
  await figure.screenshot({path:`/workspace/nessa-webhook-runtime/docs/reviews/evidence/webhook-runtime/registration/repair/${id}-${i+1}.png`});
 }
 const diagrams=await page.locator('figure.diagram').evaluateAll(fs=>fs.map(f=>({state:f.dataset.state,svgs:f.querySelectorAll('svg').length,text:f.textContent.slice(0,100)})));
 results.push({id,sourceRevision:served.document.sourceRevision,diagrams});
}
const output={rendererRevision:'c0bc0627611b100ab71a96cc6fe2fb65f1a014d8',browser:await browser.version(),results,pageErrors:errors};
writeFileSync('/workspace/nessa-webhook-runtime/docs/reviews/evidence/webhook-runtime/registration/repair/render.json',JSON.stringify(output,null,2)+'\n');
console.log(JSON.stringify(output));await browser.close();
if(errors.length ||results.some(r=>r.diagrams.some(d=>d.state==='error'||d.svgs<1)))process.exitCode=1;
