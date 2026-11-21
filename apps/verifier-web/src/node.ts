import type { JsNodeClient } from '../../../bindings/wasm/pkg/galaxy_wasm.js';
import { HASH } from './constants';
import { type Fetch, fetchText, REQUEST_TIMEOUT_MS, u32 } from './http';
import { endpoint } from './settings';

const MAX_NODE_BYTES = 256 * 1024; // Protocol §14.

/** Only instantiated for an explicitly consented lookup. Rust validates the response. */
export class BrowserNodes implements JsNodeClient {
  private readonly allowed: Set<string>;

  constructor(nodes: string[], private readonly fetcher: Fetch = fetch, private readonly timeout = REQUEST_TIMEOUT_MS) {
    this.allowed = new Set(nodes.map(url => endpoint(url, 'node')));
  }
  async lookup(base: string, photoId: string, networkId: number, limit: number, cursor?: string): Promise<string> {
    if (!this.allowed.has(base) || !HASH.test(photoId) || photoId !== photoId.toLowerCase()
        || !u32(networkId) || !u32(limit) || limit < 1 || limit > 100) {
      throw new Error('Invalid or unapproved Galaxy node lookup');
    }
    const url = new URL(`${base}/v1/photos/${photoId}`);
    url.searchParams.set('limit', String(limit));
    url.searchParams.set('network_id', String(networkId));
    if (cursor !== undefined) url.searchParams.set('cursor', cursor);
    return fetchText(this.fetcher, url.href, { headers: { accept: 'application/json' } }, MAX_NODE_BYTES, this.timeout);
  }
}
