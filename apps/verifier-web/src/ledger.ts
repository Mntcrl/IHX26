import type { JsLedgerSource } from '../../../bindings/wasm/pkg/galaxy_wasm.js';
import { type Fetch, fetchJson, MAX_RPC_BYTES, object, REQUEST_TIMEOUT_MS, u32 } from './http';
import { endpoint } from './settings';

type Params = Record<string, unknown>;
type Method = 'server_info' | 'tx' | 'ledger' | 'account_tx' | 'account_info';
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
  private readonly checkedEndpoints = new Set<number>();
  private readonly urls: string[];
  private nextId = 1;

  constructor(
    urls: string[], private readonly network: number,
    private readonly openSocket: (url: string) => Socket = url => new WebSocket(url),
    private readonly fetcher: Fetch = fetch,
    private readonly timeout = REQUEST_TIMEOUT_MS,
  ) {
    this.urls = urls.map(url => endpoint(url, 'rpc'));
  }

  tx(params: string, index: number): Promise<string> { return this.request('tx', params, index); }
  ledger_header(params: string, index: number): Promise<string> { return this.request('ledger', params, index); }
  account_root(params: string, index: number): Promise<string> { return this.request('account_info', params, index); }
  account_tx(params: string, index: number): Promise<string> { return this.request('account_tx', params, index); }
  server_info(params: string, index: number): Promise<string> { return this.request('server_info', params, index); }

  close(): void {
    for (const socket of this.sockets.values()) socket.stop('Verification finished');
    this.sockets.clear();
    this.checkedEndpoints.clear();
  }

  private async request(method: Method, json: string, index: number): Promise<string> {
    const params = object(JSON.parse(json));
    const url = this.urls[index];
    if (url === undefined) throw new Error('No XRPL endpoint configured at that index');
    try {
      if (method === 'server_info') {
        this.checkedEndpoints.delete(index);
        const probe = await this.call(url, method, params);
        const info = object(probe.info);
        const seq = object(info.validated_ledger).seq;
        if (probe.error !== undefined || (probe.status !== undefined && probe.status !== 'success')
            || info.network_id !== this.network || !u32(seq) || seq === 0
            || typeof info.complete_ledgers !== 'string') throw new Error('XRPL endpoint failed the network check');
        this.checkedEndpoints.add(index);
        return JSON.stringify(probe);
      }
      if (!this.checkedEndpoints.has(index)) throw new Error('XRPL endpoint was not checked');
      // Return command errors unchanged. Rust decides whether to restart verification.
      const result = await this.call(url, method, params);
      return JSON.stringify(result);
    } catch (error) {
      this.checkedEndpoints.delete(index);
      this.sockets.get(url)?.stop('Abandoning XRPL endpoint');
      this.sockets.delete(url);
      throw error;
    }
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
