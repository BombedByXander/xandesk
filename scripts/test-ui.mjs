import { spawn } from 'node:child_process';
import { pathToFileURL } from 'node:url';
import assert from 'node:assert/strict';
const { chromium } = await import(pathToFileURL(process.argv[3]).href);
const origin='http://127.0.0.1:18789';
const relay=spawn(process.argv[2],[],{env:{...process.env,ARCDESK_BIND:'127.0.0.1:18789',ARCDESK_ORIGINS:origin},stdio:'ignore',windowsHide:true});
const browser=await chromium.launch({executablePath:process.argv[4],headless:true});
try {
  for(let n=0;n<60;n++){try{if((await fetch(origin+'/health')).ok)break;}catch{}await new Promise(r=>setTimeout(r,100));}
  const page=await browser.newPage();const errors=[];page.on('pageerror',e=>errors.push(e.message));
  await page.goto(origin+'/');
  await page.getByRole('heading',{name:/Add your PC once/i}).waitFor();
  assert.ok((await page.locator('body').innerText()).includes('Pair this PC'));
  await page.goto(`${origin}/#/dashboard?pair=ABCD-EFGH-JKLM`);
  await page.getByRole('heading',{name:'Welcome back.'}).waitFor();
  assert.equal(await page.locator('input[name="email"]').count(),1);
  assert.ok((await page.evaluate(()=>location.hash)).includes('pair='));
  await page.goto(origin+'/viewer.html');
  assert.ok((await page.locator('body').innerText()).includes('sign in'));
  assert.equal(await page.locator('#pair-code').count(),0);
  for(const width of [390,768,1440]) {
    await page.setViewportSize({width,height:900});
    assert.ok(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth),`Horizontal overflow at ${width}px`);
  }
  assert.deepEqual(errors,[]);
  console.log('PASS: owner dashboard pairing-code handoff, sign-in gate, peer viewer routed through account, responsive layouts.');
} finally {await browser.close();relay.kill();}