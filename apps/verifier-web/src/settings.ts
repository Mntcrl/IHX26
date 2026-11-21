export interface Settings {
  rpcEndpoints: string[];
  lookupNode: string;
  explorerTx: string;
  explorerLedger: string;
}

const PATH = /^\/[A-Za-z0-9/.~_-]*$/;
// CSP host sources do not support literal IPv6 addresses in Chromium.
const HOST = /^[a-zA-Z0-9.\-]+(?::[0-9]+)?$/;

export function endpoint(raw: string, kind: 'rpc' | 'node'): string {
  const url = new URL(raw);
  const secure = kind === 'rpc' ? ['wss:', 'https:'] : ['https:'];
  const local = kind === 'rpc' ? ['ws:', 'http:'] : ['http:'];
  const loopback = url.hostname === 'localhost' || url.hostname === '127.0.0.1';
  if ((!secure.includes(url.protocol) && !(loopback && local.includes(url.protocol)))
      || url.username || url.password || url.search || url.hash || !HOST.test(url.host) || !PATH.test(url.pathname)) {
    throw new Error(`${kind === 'rpc' ? 'XRPL endpoint' : 'Galaxy node'} must use ${secure.join(' or ')} with a hostname or IPv4 address and without credentials, query or fragment (plain HTTP/WS only on localhost or 127.0.0.1).`);
  }
  if (kind === 'node') return `${url.origin}${url.pathname.replace(/\/+$/, '')}`;
  return url.href;
}

export function explorerTemplate(raw: string, placeholder: '{hash}' | '{index}'): string {
  const template = raw.trim();
  if (!template) return '';
  const parts = template.split(placeholder);
  if (parts.length !== 2 || !/^https:\/\/[^/?#]+[/?#]/i.test(parts[0])) {
    throw new Error(`Explorer URL must be HTTPS with one ${placeholder} outside the authority.`);
  }
  const url = new URL(parts.join('1'));
  if (url.protocol !== 'https:' || url.username || url.password || !HOST.test(url.host)) {
    throw new Error('Explorer URL must be HTTPS without credentials.');
  }
  return template;
}

/** Query overrides are HTTPS origins only; paths, credentials and duplicates are ignored. */
export function selectNode(fallback: string, search: string): string {
  const overrides = new URLSearchParams(search).getAll('node');
  const override = overrides[0];
  if (overrides.length === 1 && /^https:\/\/[a-z0-9.-]+(?::[0-9]+)?\/?$/i.test(override)) {
    try { return endpoint(override, 'node'); }
    catch { /* The exact default host is disclosed in the UI. */ }
  }
  return endpoint(fallback, 'node');
}

export function loadSettings(): Settings {
  return {
    rpcEndpoints: [...new Set(__DEPLOYMENT__.endpoints.map(url => endpoint(url, 'rpc')))],
    lookupNode: selectNode(__DEPLOYMENT__.lookupNode, location.search),
    explorerTx: explorerTemplate(__DEPLOYMENT__.explorerTx, '{hash}'),
    explorerLedger: explorerTemplate(__DEPLOYMENT__.explorerLedger, '{index}'),
  };
}

/** Install before reading a file or creating a worker; the Blob worker inherits this policy. */
export function installNetworkPolicy(settings: Settings, wasm: string): void {
  const sources = new Set([wasm]);
  for (const url of settings.rpcEndpoints) {
    // Omit the root slash for WebSockets: connect-src path matching is not
    // consistent for upgrades across engines; non-root paths remain explicit.
    sources.add(url.replace(/^(wss?:\/\/[^/]+)\/$/, '$1'));
  }
  sources.add(`${settings.lookupNode}/v1/photos/`);
  const meta = document.createElement('meta');
  meta.httpEquiv = 'Content-Security-Policy';
  meta.content = `connect-src ${[...sources].join(' ')}`;
  document.head.append(meta);
}
