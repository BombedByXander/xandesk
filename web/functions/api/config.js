export async function onRequest({ env }) {
  const value = String(env.XANDESK_API || 'https://xandesk.onrender.com').replace(/\/$/, '');
  const relayValue = String(env.XANDESK_RELAY || env.XANDESK_API || '').replace(/\/$/, '');
  const supabaseValue = String(env.XANDESK_SUPABASE_URL || '').replace(/\/$/, '');
  const publishableKey = String(env.XANDESK_SUPABASE_PUBLISHABLE_KEY || '');
  let apiBase = '';
  let relayBase = '';
  let supabaseUrl = '';
  try {
    const url = new URL(value);
    if (url.protocol === 'https:' || (url.protocol === 'http:' && ['localhost', '127.0.0.1'].includes(url.hostname))) apiBase = url.origin;
  } catch {}
  try {
    const url = new URL(relayValue);
    if (url.protocol === 'https:' || (url.protocol === 'http:' && ['localhost', '127.0.0.1'].includes(url.hostname))) relayBase = url.origin;
  } catch {}
  try {
    const url = new URL(supabaseValue);
    if (url.protocol === 'https:') supabaseUrl = url.origin;
  } catch {}
  return Response.json({ apiBase, relayBase, supabaseUrl, supabasePublishableKey: publishableKey }, { headers: { 'Cache-Control': 'no-store' } });
}
