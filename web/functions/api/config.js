export async function onRequest({ env }) {
  const value = String(env.XANDESK_API || '').replace(/\/$/, '');
  let apiBase = '';
  try {
    const url = new URL(value);
    if (url.protocol === 'https:' || (url.protocol === 'http:' && ['localhost', '127.0.0.1'].includes(url.hostname))) apiBase = url.origin;
  } catch {}
  return Response.json({ apiBase }, { headers: { 'Cache-Control': 'no-store' } });
}
