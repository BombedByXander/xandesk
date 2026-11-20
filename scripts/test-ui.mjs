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
  await page.goto(`${origin}/#room=${'a'.repeat(32)}&token=${'b'.repeat(64)}`);
  await page.waitForFunction(()=>!location.hash);
  assert.equal(await page.locator('#server').inputValue(),origin);
  assert.equal(await page.evaluate(()=>localStorage.getItem('arcdesk-server')),origin);
  assert.equal(await page.locator('#room').inputValue(),'a'.repeat(32));
  assert.equal(await page.locator('#token').inputValue(),'b'.repeat(64));
  assert.equal(await page.evaluate(()=>Object.keys(localStorage).filter(k=>k!=='arcdesk-server').length),0);
  await page.locator('#connect').click();
  await page.waitForFunction(()=>document.querySelector('#status').textContent.includes('unavailable'));
  assert.equal(await page.locator('#connect').isEnabled(),true);
  await page.locator('#server').fill('https://relay.example.net/custom/path');
  await page.locator('#server').press('Tab');
  assert.equal(await page.evaluate(()=>localStorage.getItem('arcdesk-server')),'https://relay.example.net');
  await page.locator('#server').fill('http://192.168.1.5:8787');
  await page.locator('#connect').click();
  await page.waitForFunction(()=>document.querySelector('#status').textContent.includes('require HTTPS'));
  for(const width of [390,768,1440]) {
    await page.setViewportSize({width,height:900});
    assert.ok(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth),`Horizontal overflow at ${width}px`);
  }
  assert.deepEqual(errors,[]);
  console.log('PASS: browser loads, fragment credentials clear, relay preference persists without pairing secrets, connection errors recover, remote HTTP rejected, mobile/tablet/desktop layouts fit.');
} finally {await browser.close();relay.kill();}
