const $ = id => document.getElementById(id);
let ws, pc, input, statsTimer, connectTimer, heartbeatTimer, generation = 0;
let previousStats, lastFrameTime = performance.now(), presentedFrames = 0, presentationFps = 0;
const pressedKeys = new Set();
const screen = $('screen');
const relayPreferenceKey='xandesk-relay';
function normalizeRelay(value){const u=new URL(value);if(!['https:','http:'].includes(u.protocol))throw new Error('Use an HTTPS relay URL.');if(u.protocol==='http:'&&!['localhost','127.0.0.1','[::1]'].includes(u.hostname))throw new Error('Remote relay connections require HTTPS.');u.pathname='/';u.search='';u.hash='';return u.origin;}
let configuredRelay='';
const invitation = new URLSearchParams(location.hash.slice(1));
if(invitation.has('pair'))$('pair-code').value=invitation.get('pair');
const legacyRoom=invitation.get('room'), legacyToken=invitation.get('token');
if (location.hash) history.replaceState(null, '', location.pathname + location.search);
const status = message => { $('status').textContent = message; };
const signal = packet => { if (ws?.readyState === WebSocket.OPEN) ws.send(JSON.stringify(packet)); };
const send = packet => {
  if (input?.readyState === 'open' && input.bufferedAmount < 65536) input.send(JSON.stringify(packet));
};
function release() { send({type:'release'}); pressedKeys.clear(); }
function disconnect(message = 'Disconnected. Your PC is no longer being controlled.') {
  generation++; release(); clearInterval(statsTimer); clearTimeout(connectTimer); clearInterval(heartbeatTimer);
  if (document.pointerLockElement) document.exitPointerLock();
  if (ws) { ws.onclose = null; ws.close(); ws = null; }
  if (pc) { pc.onconnectionstatechange = null; pc.close(); pc = null; }
  input = null; screen.srcObject = null; previousStats = null;
  $('session').hidden = true; $('connect-panel').hidden = false; $('connect').disabled = false;
  $('waiting').hidden = false; status(message);
}
function waitForIce(connection) {
  return new Promise((resolve, reject) => {
    if (connection.iceGatheringState === 'complete') return resolve();
    const timeout = setTimeout(() => { cleanup(); reject(new Error('ICE gathering timed out. Check the STUN/TURN configuration.')); }, 20000);
    const changed = () => { if (connection.iceGatheringState === 'complete') { cleanup(); resolve(); } };
    const cleanup = () => { clearTimeout(timeout); connection.removeEventListener('icegatheringstatechange', changed); };
    connection.addEventListener('icegatheringstatechange', changed);
  });
}
async function makeOffer(iceServers, current) {
  const connection = new RTCPeerConnection({iceServers,iceTransportPolicy:'all'}); pc = connection;
  connection.addTransceiver('video', {direction:'recvonly'});
  input = connection.createDataChannel('control', {ordered:true});
  input.onopen = () => { status('Connected. Click the screen to control your PC.'); screen.focus(); heartbeatTimer = setInterval(() => send({type:'heartbeat'}), 1000); };
  connection.ontrack = e => { screen.srcObject = e.streams[0] || new MediaStream([e.track]); screen.play().catch(() => {}); };
  connection.onconnectionstatechange = () => {
    if (current !== generation) return;
    if (connection.connectionState === 'connected') {
      clearTimeout(connectTimer); $('waiting').hidden = true;
      statsTimer = setInterval(() => updateStats(connection).catch(() => {}), 1000);
    }
    if (['failed','closed','disconnected'].includes(connection.connectionState)) disconnect('Connection lost. Reconnect to request access again. A TURN server may be needed on this network.');
  };
  await connection.setLocalDescription(await connection.createOffer());
  await waitForIce(connection);
  if (current === generation) signal({type:'offer', sdp:connection.localDescription.sdp});
}
const relayConfigReady=fetch('/api/config').then(r=>r.ok?r.json():{}).then(config=>{
  const candidate=config.relayBase||(legacyRoom&&legacyToken?location.origin:'')||configuredRelay||localStorage.getItem(relayPreferenceKey)||(['localhost','127.0.0.1'].includes(location.hostname)?location.origin:'');
  if(candidate)configuredRelay=normalizeRelay(candidate);
}).catch(()=>{if(['localhost','127.0.0.1'].includes(location.hostname))configuredRelay=location.origin;});
$('pair-form').addEventListener('submit', async e => {
  e.preventDefault();
  if (ws) disconnect();
  const current = ++generation;
  try {
    await relayConfigReady;
    if(!configuredRelay)throw new Error('XanDesk is not connected to its public relay yet. Set XANDESK_RELAY in your Pages project after deploying the relay server.');
    const code=$('pair-code').value.toUpperCase().replace(/[^A-Z0-9]/g,'');
    if(!(legacyRoom&&legacyToken)&&!/^[A-HJ-NP-Z2-9]{12}$/.test(code))throw new Error('Enter the 12-character pairing code shown in the XanDesk Windows app.');
    const endpoint = new URL(configuredRelay);
    endpoint.protocol = endpoint.protocol === 'https:' ? 'wss:' : 'ws:'; endpoint.pathname = '/ws'; endpoint.search = ''; endpoint.hash = '';
    $('connect').disabled = true; status('Requesting permission from your PC…');
    ws = new WebSocket(endpoint);
    connectTimer = setTimeout(() => disconnect('Connection request expired. Check the host app and try again.'), 120000);
    ws.onopen = () => signal(legacyRoom&&legacyToken?{type:'join',room:legacyRoom,token:legacyToken}:{type:'pair',pairing_code:code});
    ws.onerror = () => disconnect('Could not reach the connection server. Check its address, HTTPS, and allowed origins.');
    ws.onclose = () => disconnect('The host or connection server disconnected.');
    ws.onmessage = async event => {
      if (current !== generation) return;
      try {
        const message = JSON.parse(event.data);
        if (message.type === 'ready') {
          $('connect-panel').hidden = true; $('session').hidden = false; status('Approved. Establishing encrypted video and controls…');
          clearTimeout(connectTimer); connectTimer = setTimeout(() => disconnect('ICE could not connect through the configured STUN/TURN servers. Check TURN settings or network restrictions, then retry.'), 125000);
          await makeOffer(message.ice_servers, current);
        } else if (message.type === 'answer' && pc) await pc.setRemoteDescription({type:'answer',sdp:message.sdp});
        else if (message.type === 'reject') disconnect('The connection was declined at the PC.');
        else if (message.type === 'left') disconnect('The host stopped sharing.');
        else if (message.type === 'error') disconnect(message.message);
      } catch (error) { if (current === generation) disconnect(error.message); }
    };
  } catch (error) { disconnect(error.message); }
});
async function updateStats(connection) {
  const stats = await connection.getStats();
  let video;
  stats.forEach(report => {
    if (report.type === 'inbound-rtp' && report.kind === 'video') video = report;
    if (report.type === 'candidate-pair' && report.state === 'succeeded' && report.nominated && report.currentRoundTripTime != null) $('rtt').textContent = `${Math.round(report.currentRoundTripTime*1000)} ms RTT`;
  });
  if (!video) return;
  if (previousStats && video.timestamp > previousStats.timestamp) {
    const seconds = (video.timestamp - previousStats.timestamp) / 1000;
    const fps = Math.max(0, (video.framesDecoded - previousStats.framesDecoded) / seconds);
    $('fps').textContent = `${fps.toFixed(1)} FPS`;
    $('fps').title = `Decoded frames per second; presented: ${presentationFps.toFixed(1)} FPS`;
    $('bitrate').textContent = `${Math.max(0,(video.bytesReceived-previousStats.bytesReceived)*8/seconds/1e6).toFixed(1)} Mbps`;
  }
  $('resolution').textContent = `${video.frameWidth || screen.videoWidth} × ${video.frameHeight || screen.videoHeight}`;
  previousStats = video;
}
if ('requestVideoFrameCallback' in screen) {
  const frame = now => {
    presentedFrames++;
    if (now-lastFrameTime >= 1000) { presentationFps = presentedFrames*1000/(now-lastFrameTime); presentedFrames = 0; lastFrameTime = now; }
    screen.requestVideoFrameCallback(frame);
  }; screen.requestVideoFrameCallback(frame);
}
function position(event) {
  const r = screen.getBoundingClientRect();
  if (!screen.videoWidth) return null;
  const scale = Math.min(r.width/screen.videoWidth, r.height/screen.videoHeight);
  const width = screen.videoWidth*scale, height = screen.videoHeight*scale;
  const x = (event.clientX-r.left-(r.width-width)/2)/width;
  const y = (event.clientY-r.top-(r.height-height)/2)/height;
  return x>=0 && x<=1 && y>=0 && y<=1 ? {type:'move', x, y} : null;
}
screen.addEventListener('pointermove', e => {
  if (document.pointerLockElement === screen) send({type:'relative',x:e.movementX,y:e.movementY});
  else { const p = position(e); if (p) send(p); }
});
screen.addEventListener('pointerdown', e => { e.preventDefault(); screen.focus(); screen.setPointerCapture(e.pointerId); const p = position(e); if (p) { send(p); send({type:'button',button:e.button,down:true}); } });
screen.addEventListener('pointerup', e => { e.preventDefault(); send({type:'button',button:e.button,down:false}); });
screen.addEventListener('pointercancel', release);
screen.addEventListener('contextmenu', e => e.preventDefault());
screen.addEventListener('wheel', e => { e.preventDefault(); send({type:'wheel',delta:Math.sign(e.deltaY)*-120}); }, {passive:false});
screen.addEventListener('keydown', e => {
  if (!input || e.code === 'Escape' && document.pointerLockElement) return;
  e.preventDefault(); if (!pressedKeys.has(e.code)) { pressedKeys.add(e.code); send({type:'key',code:e.code,down:true}); }
});
screen.addEventListener('keyup', e => { e.preventDefault(); pressedKeys.delete(e.code); send({type:'key',code:e.code,down:false}); });
screen.addEventListener('blur', release); window.addEventListener('blur', release);
document.addEventListener('visibilitychange', () => { if (document.hidden) release(); });
document.addEventListener('pointerlockchange', release);
window.addEventListener('beforeunload', () => disconnect());
$('disconnect').onclick = () => disconnect();
$('fullscreen').onclick = () => $('stage').requestFullscreen().catch(error => status(error.message));
$('lock').onclick = async () => { try { screen.focus(); await screen.requestPointerLock(); } catch { status('Game mouse is not supported in this browser.'); } };
$('escape').onclick = () => { send({type:'key',code:'Escape',down:true}); send({type:'key',code:'Escape',down:false}); };
$('paste').onclick = async () => {
  try { const text = await navigator.clipboard.readText(); if (text.length > 16000) throw new Error('Clipboard text must be under 16,000 characters.'); send({type:'clipboard',text}); status('Text copied to the PC clipboard. Press Ctrl+V on the remote screen to paste.'); }
  catch (error) { status(`Clipboard: ${error.message}`); }
};
$('keyboard').onclick = () => { $('phone-input').hidden = !$('phone-input').hidden; if (!$('phone-input').hidden) $('phone-input').focus(); };
$('phone-input').addEventListener('keydown', e => {
  if (e.key === 'Enter' && e.target.value) {
    send({type:'clipboard',text:e.target.value.slice(0,16000)});
    send({type:'key',code:'ControlLeft',down:true}); send({type:'key',code:'KeyV',down:true});
    send({type:'key',code:'KeyV',down:false}); send({type:'key',code:'ControlLeft',down:false}); e.target.value='';
  }
});
