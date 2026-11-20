// Read-only local WebRTC test. Does not transmit remote keyboard/mouse input.
// Usage: node scripts/test-browser.mjs <absolute-path-to-playwright-module> <chrome-or-edge-exe>
import assert from 'node:assert/strict';
import { pathToFileURL } from 'node:url';
const { chromium } = await import(pathToFileURL(process.argv[2]).href);
const browser = await chromium.launch({executablePath:process.argv[3],headless:true});
try {
  const page=await browser.newPage();
  const result=await page.evaluate(async()=>{
    const ws=new WebSocket('ws://127.0.0.1:18788');
    await new Promise((resolve,reject)=>{ws.onopen=resolve;ws.onerror=reject;});
    const pc=new RTCPeerConnection({iceServers:[]});
    pc.addTransceiver('video',{direction:'recvonly'});
    const controls=pc.createDataChannel('control',{ordered:true}); // Never send input during this test.
    const video=document.createElement('video');video.autoplay=true;video.muted=true;document.body.append(video);
    pc.ontrack=e=>{video.srcObject=new MediaStream([e.track]);video.play();};
    const response=new Promise(resolve=>{ws.onmessage=e=>resolve(JSON.parse(e.data));});
    await pc.setLocalDescription(await pc.createOffer());
    if(pc.iceGatheringState!=='complete')await new Promise(resolve=>{pc.onicegatheringstatechange=()=>{if(pc.iceGatheringState==='complete')resolve();};});
    ws.send(JSON.stringify({type:'offer',sdp:pc.localDescription.sdp}));
    await pc.setRemoteDescription(await response);
    const start=performance.now();let report={framesDecoded:0};
    while(performance.now()-start<45000){
      await new Promise(resolve=>setTimeout(resolve,1000));
      (await pc.getStats()).forEach(s=>{if(s.type==='inbound-rtp'&&s.kind==='video')report={framesDecoded:s.framesDecoded||0,width:video.videoWidth,height:video.videoHeight,bytesReceived:s.bytesReceived,connection:pc.connectionState,control:controls.readyState};});
      if(report.framesDecoded>=3&&report.width>0)break;
      if(pc.connectionState==='failed')break;
    }
    ws.send(JSON.stringify(report));await new Promise(resolve=>setTimeout(resolve,200));pc.close();ws.close();return report;
  });
  assert.ok(result.framesDecoded>0,JSON.stringify(result));assert.ok(result.width>0);assert.equal(result.control,'open');
  console.log('PASS: real Windows capture → Rust H.264 → encrypted WebRTC → browser decode:',JSON.stringify(result));
} finally {await browser.close();}
