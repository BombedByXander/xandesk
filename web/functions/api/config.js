export async function onRequest({ env }) {
  const value = String(env.XANDESK_API || '').replace(/\/$/, '');
  const relayValue = String(env.XANDESK_RELAY || '').replace(/\/$/, '');
  let apiBase = '';
  let relayBase = '';
  try {
    const url = new URL(value);
    if (url.protocol === 'https:' || (url.protocol === 'http:' && ['localhost', '127.0.0.1'].includes(url.hostname))) apiBase = url.origin;
  } catch {}
  try {
    const url = new URL(relayValue);
    if (url.protocol === 'https:' || (url.protocol === 'http:' && ['localhost', '127.0.0.1'].includes(url.hostname))) relayBase = url.origin;
  } catch {}
  return Response.json({ apiBase, relayBase }, { headers: { 'Cache-Control': 'no-store' } });
}
