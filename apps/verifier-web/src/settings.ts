import { MAX_ENDPOINTS } from './constants';

export interface Settings {
  rpcEndpoints: string[];
  nodes: string[];
  discovery: boolean;
  explorerTx: string;
  explorerLedger: string;
}

const SETTINGS_KEY = 'galaxy.verifier.settings.v1';
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

export function validateSettings(input: Settings): Settings {
  if (!Array.isArray(input.rpcEndpoints) || !input.rpcEndpoints.length || input.rpcEndpoints.length > MAX_ENDPOINTS
      || !Array.isArray(input.nodes) || input.nodes.length > MAX_ENDPOINTS || typeof input.discovery !== 'boolean') {
    throw new Error('Configure one to eight XRPL endpoints and at most eight Galaxy nodes.');
  }
  return {
    rpcEndpoints: [...new Set(input.rpcEndpoints.map(url => endpoint(url, 'rpc')))],
    nodes: [...new Set(input.nodes.map(url => endpoint(url, 'node')))],
    discovery: input.discovery,
    explorerTx: explorerTemplate(input.explorerTx, '{hash}'),
    explorerLedger: explorerTemplate(input.explorerLedger, '{index}'),
  };
}

export function loadSettings(): Settings {
  try {
    const stored = localStorage.getItem(SETTINGS_KEY);
    if (stored) return validateSettings(JSON.parse(stored));
  } catch { /* Invalid or unavailable storage does not weaken defaults. */ }
  return validateSettings({
    rpcEndpoints: __DEPLOYMENT__.endpoints,
    nodes: [], discovery: false,
    explorerTx: __DEPLOYMENT__.explorerTx,
    explorerLedger: __DEPLOYMENT__.explorerLedger,
  });
}

export function storeSettings(settings: Settings): void {
  localStorage.setItem(SETTINGS_KEY, JSON.stringify(validateSettings(settings)));
}

/** Install before any file is read or any worker runs. On changes, reload to replace the policy. */
export function installNetworkPolicy(settings: Settings, wasm: string): void {
  const sources = new Set([wasm]);
  for (const url of settings.rpcEndpoints) {
    // Omit the root slash for WebSockets: connect-src path matching is not
    // consistent for upgrades across engines; non-root paths remain explicit.
    sources.add(url.replace(/^(wss?:\/\/[^/]+)\/$/, '$1'));
  }
  if (settings.discovery) {
    for (const node of settings.nodes) sources.add(`${node}/v1/photos/`);
  }
  const meta = document.createElement('meta');
  meta.httpEquiv = 'Content-Security-Policy';
  meta.content = `connect-src ${[...sources].join(' ')}`;
  document.head.append(meta);
}
