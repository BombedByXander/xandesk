import { spawn } from 'node:child_process';
import assert from 'node:assert/strict';
import { randomBytes } from 'node:crypto';

const executable = process.argv[2];
if (!executable) throw new Error('Usage: node scripts/test-relay.mjs <path-to-arcdesk-relay.exe>');
const port = 18787, address = `http://127.0.0.1:${port}`;
const child = spawn(executable, [], { env: {...process.env, ARCDESK_BIND:`127.0.0.1:${port}`, ARCDESK_ORIGINS:address}, stdio:['ignore','pipe','pipe'], windowsHide:true });
let logs = ''; child.stdout.on('data', b => { logs += b; }); child.stderr.on('data', b => { logs += b; });
const delay = ms => new Promise(resolve => setTimeout(resolve, ms));
const sockets = [];
async function socket() {
  const ws = new WebSocket(`ws://127.0.0.1:${port}/ws`); sockets.push(ws);
  ws.queue = []; ws.waiter = null;
  ws.addEventListener('message', e => { const data=JSON.parse(e.data); if(ws.waiter) {const deliver=ws.waiter;ws.waiter=null;deliver(data);} else ws.queue.push(data); });
  await new Promise((resolve,reject) => { ws.onopen=resolve;ws.onerror=reject; });
  ws.packet = packet => ws.send(JSON.stringify(packet));
  ws.next = (timeout=3000) => ws.queue.length ? Promise.resolve(ws.queue.shift()) : new Promise((resolve,reject) => {
    const timer=setTimeout(()=>{ws.waiter=null;reject(new Error('Signal timeout'));},timeout);
    ws.waiter=data=>{clearTimeout(timer);resolve(data);};
  });
  return ws;
}
try {
  let healthy=false;
  for(let n=0;n<60;n++){ try { if((await fetch(`${address}/health`)).ok){healthy=true;break;} }catch{} await delay(100); }
  assert.ok(healthy,`Relay did not start: ${logs}`);
  const page=await fetch(address);assert.equal(page.status,200);assert.ok((await page.text()).includes('XanDesk'));assert.equal(page.headers.get('referrer-policy'),'no-referrer');
  const room=randomBytes(16).toString('hex'),token=randomBytes(32).toString('hex');
  const host=await socket();host.packet({type:'register',room,token});assert.equal((await host.next()).type,'registered');
  const wrong=await socket();wrong.packet({type:'join',room,token:randomBytes(32).toString('hex')});assert.equal((await wrong.next()).type,'error');
  const collision=await socket();collision.packet({type:'register',room,token});assert.equal((await collision.next()).type,'error');
  const viewer=await socket();viewer.packet({type:'join',room,token});assert.equal((await host.next()).type,'request');
  viewer.packet({type:'offer',sdp:'must-not-forward-before-approval'});await delay(150);assert.equal(host.queue.length,0);
  const second=await socket();second.packet({type:'join',room,token});assert.equal((await second.next()).type,'error');
  host.packet({type:'approve'});assert.equal((await viewer.next()).type,'ready');
  viewer.packet({type:'offer',sdp:'test-offer'});assert.deepEqual(await host.next(),{type:'offer',sdp:'test-offer'});
  host.packet({type:'answer',sdp:'test-answer'});assert.deepEqual(await viewer.next(),{type:'answer',sdp:'test-answer'});
  viewer.close();assert.equal((await host.next()).type,'left');
  const declined=await socket();declined.packet({type:'join',room,token});assert.equal((await host.next()).type,'request');host.packet({type:'reject'});assert.equal((await declined.next()).type,'reject');assert.equal((await host.next()).type,'left');
  const last=await socket();last.packet({type:'join',room,token});assert.equal((await host.next()).type,'request');host.close();assert.equal((await last.next()).type,'left');
  console.log('PASS: serving, security headers, authentication, room ownership, approval gate, single viewer, SDP forwarding, rejection, viewer cleanup, host shutdown.');
} finally { for(const ws of sockets)ws.close();child.kill(); }

