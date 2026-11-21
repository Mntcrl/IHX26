import type { JsLedgerSource } from '../../../bindings/wasm/pkg/galaxy_wasm.js';
import { type Fetch, fetchJson, MAX_RPC_BYTES, object, REQUEST_TIMEOUT_MS, u32 } from './http';
import { endpoint } from './settings';

type Params = Record<string, unknown>;
type Method = 'server_info' | 'tx' | 'ledger' | 'account_tx';
type Socket = Pick<WebSocket, 'readyState' | 'addEventListener' | 'send' | 'close'>;
type Pending = { resolve: (payload: Params) => void; reject: (error: Error) => void; timer: number };

class LedgerSocket {
  readonly opened: Promise<void>;
  private readonly pending = new Map<number, Pending>();

  constructor(private readonly socket: Socket, private readonly timeout: number) {
    const { promise, resolve, reject } = Promise.withResolvers<void>();
    this.opened = promise;
    const timer = setTimeout(() => {
      reject(new Error('XRPL WebSocket connect timed out'));
      this.stop('XRPL WebSocket connect timed out');
    }, timeout);
    socket.addEventListener('open', () => { clearTimeout(timer); resolve(); }, { once: true });
    const closed = () => {
      clearTimeout(timer);
      reject(new Error('XRPL WebSocket did not open'));
      this.stop('XRPL WebSocket disconnected');
    };
    socket.addEventListener('close', closed, { once: true });
    socket.addEventListener('error', closed, { once: true });
    socket.addEventListener('message', event => this.message(event.data));
  }

  get open(): boolean { return this.socket.readyState === 1; }

  async request(id: number, method: Method, params: Params): Promise<Params> {
    await this.opened;
    if (!this.open) throw new Error('XRPL WebSocket is not open');
    const { promise, resolve, reject } = Promise.withResolvers<Params>();
    const timer = setTimeout(() => this.stop('XRPL WebSocket request timed out'), this.timeout);
    this.pending.set(id, { resolve, reject, timer });
    try { this.socket.send(JSON.stringify({ ...params, command: method, id })); }
    catch { this.stop('Could not send XRPL request'); }
    return promise;
  }

  stop(reason: string): void {
    for (const { reject, timer } of this.pending.values()) {
      clearTimeout(timer);
      reject(new Error(reason));
    }
    this.pending.clear();
    if (this.socket.readyState < 2) this.socket.close();
  }

  private message(data: unknown): void {
    try {
      if (typeof data !== 'string' || data.length > MAX_RPC_BYTES
          || (data.length * 3 > MAX_RPC_BYTES && new Blob([data]).size > MAX_RPC_BYTES)) {
        throw new Error('Non-text or oversized XRPL response');
      }
      const msg = object(JSON.parse(data));
      if (msg.type !== 'response' || typeof msg.id !== 'number') return;
      const call = this.pending.get(msg.id);
      if (!call) return;
      const result = msg.status === 'success' ? object(msg.result)
        : msg.status === 'error' && typeof msg.error === 'string' ? { error: msg.error }
        : null;
      if (result === null) throw new Error('Malformed XRPL WebSocket response');
      clearTimeout(call.timer);
      this.pending.delete(msg.id);
      call.resolve(result);
    } catch (error) {
      this.stop(error instanceof Error && !(error instanceof SyntaxError) ? error.message : 'Invalid XRPL response JSON');
    }
  }
}

export class BrowserLedger implements JsLedgerSource {
  private readonly sockets = new Map<string, LedgerSocket>();
  private readonly urls: string[];
  private nextId = 1;
  private preferred = 0;

  constructor(
    urls: string[], private readonly network: number,
    private readonly openSocket: (url: string) => Socket = url => new WebSocket(url),
    private readonly fetcher: Fetch = fetch,
    private readonly timeout = REQUEST_TIMEOUT_MS,
  ) {
    this.urls = urls.map(url => endpoint(url, 'rpc'));
  }

  tx(params: string): Promise<string> { return this.request('tx', params); }
  ledger_header(params: string): Promise<string> { return this.request('ledger', params); }
  account_tx(params: string): Promise<string> { return this.request('account_tx', params); }
  server_info(params: string): Promise<string> { return this.request('server_info', params); }

  close(): void {
    for (const socket of this.sockets.values()) socket.stop('Verification finished');
    this.sockets.clear();
  }

  private async request(method: Method, json: string): Promise<string> {
    const params = object(JSON.parse(json));
    const preflight = { api_version: params.api_version };
    let lastError = new Error('No XRPL endpoint configured');
    for (let offset = 0; offset < this.urls.length; offset++) {
      const index = (this.preferred + offset) % this.urls.length;
      const url = this.urls[index];
      try {
        // Pin the network before sending a transaction hash or account query.
        const probe = await this.call(url, 'server_info', preflight);
        const info = object(probe.info);
        const seq = object(info.validated_ledger).seq;
        if (probe.error !== undefined || (probe.status !== undefined && probe.status !== 'success')
            || info.network_id !== this.network || !u32(seq) || seq === 0
            || typeof info.complete_ledgers !== 'string') throw new Error('XRPL endpoint failed the network check');
        // An XRPL command-level error (including txnNotFound) is a valid response.
        const result = method === 'server_info' ? probe : await this.call(url, method, params);
        this.preferred = index;
        return JSON.stringify(result);
      } catch (error) {
        lastError = error instanceof Error ? error : new Error('XRPL request failed');
        this.sockets.get(url)?.stop('Abandoning XRPL endpoint');
        this.sockets.delete(url);
      }
    }
    throw lastError;
  }

  private async call(url: string, method: Method, params: Params): Promise<Params> {
    const id = this.nextId++;
    if (url.startsWith('ws')) {
      let socket = this.sockets.get(url);
      // Never silently replace a disconnected socket between preflight and the query.
      if (!socket && method === 'server_info') {
        socket = new LedgerSocket(this.openSocket(url), this.timeout);
        this.sockets.set(url, socket);
      }
      if (!socket) throw new Error('XRPL WebSocket was not checked');
      return socket.request(id, method, params);
    }
    const envelope = await fetchJson(this.fetcher, url, {
      method: 'POST', headers: { 'content-type': 'application/json' },
      body: JSON.stringify({ id, method, params: [params] }),
    }, MAX_RPC_BYTES, this.timeout);
    // XRPL's HTTP endpoint may omit id for the legacy JSON-RPC envelope.
    if (envelope.error !== undefined || (envelope.id !== undefined && envelope.id !== id)) {
      throw new Error('Unexpected XRPL JSON-RPC envelope');
    }
    return object(envelope.result);
  }
}
