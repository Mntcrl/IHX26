import { describe, expect, it, vi } from 'vitest';
import { BrowserLedger } from '../src/ledger';
import { BrowserNodes } from '../src/node';
import { endpoint, explorerTemplate, validateSettings } from '../src/settings';

const GOOD = 'wss://good.example/';
const BAD = 'wss://bad.example/';
const RPC = 'https://rpc.example/';
const NODE = 'https://node.example/galaxy';
const INFO = { info: { network_id: 1, validated_ledger: { seq: 123 }, complete_ledgers: '1-123' } };
const PARAMS = '{"transaction":"AA","api_version":2,"binary":true}';
type Request = { id: number; command: string; transaction?: string; api_version: number };

class MockSocket extends EventTarget {
  readyState = 0;
  sent: Request[] = [];

  constructor(private reply: (req: Request, sock: MockSocket) => void, open = true) {
    super();
    if (open) queueMicrotask(() => {
      this.readyState = 1;
      this.dispatchEvent(new Event('open'));
    });
  }

  send(data: string): void {
    const req = JSON.parse(data);
    this.sent.push(req);
    queueMicrotask(() => this.reply(req, this));
  }

  close(): void {
    this.readyState = 3;
    this.dispatchEvent(new Event('close'));
  }

  receive(data: unknown): void { this.dispatchEvent(new MessageEvent('message', { data })); }
  response(id: number, result: unknown): void {
    this.receive(JSON.stringify({ id, type: 'response', status: 'success', result }));
  }
}

const good = (req: Request, socket: MockSocket) => {
  if (req.command === 'server_info') socket.response(req.id, INFO);
  else {
    socket.response(req.id + 100, { wrong: true });
    socket.response(req.id, { transaction: req.transaction });
  }
};

describe('XRPL transport', () => {
  it('matches response IDs and fails over without disclosing hashes to the wrong network', async () => {
    const bad = new MockSocket((req, sock) => sock.response(req.id, { info: { ...INFO.info, network_id: 0 } }));
    let ok: MockSocket | undefined;
    const ledger = new BrowserLedger([BAD, GOOD], 1, url => url === BAD ? bad : (ok = new MockSocket(good)));
    expect(JSON.parse(await ledger.tx(PARAMS))).toEqual({ transaction: 'AA' });
    expect(bad.sent).toEqual([{ id: 1, command: 'server_info', api_version: 2 }]);
    expect(ok?.sent).toEqual([
      { id: 2, command: 'server_info', api_version: 2 },
      { id: 3, command: 'tx', api_version: 2, transaction: 'AA', binary: true },
    ]);
    ledger.close();
    expect(ok?.readyState).toBe(3);
  });

  it('preserves XRPL command errors for the Rust verifier', async () => {
    const socket = new MockSocket((req, sock) => {
      if (req.command === 'server_info') sock.response(req.id, INFO);
      else sock.receive(JSON.stringify({ id: req.id, type: 'response', status: 'error', error: 'txnNotFound' }));
    });
    const ledger = new BrowserLedger([GOOD], 1, () => socket);
    expect(JSON.parse(await ledger.tx(PARAMS))).toEqual({ error: 'txnNotFound' });
    ledger.close();
  });

  it('times out a socket that neither opens nor closes', async () => {
    vi.useFakeTimers();
    try {
      const sock = new MockSocket(good, false);
      const ledger = new BrowserLedger([GOOD], 1, () => sock, fetch, 50);
      const rejection = expect(ledger.tx(PARAMS)).rejects.toThrow('connect timed out');
      await vi.advanceTimersByTimeAsync(50);
      await rejection;
      expect(sock.sent).toEqual([]);
    } finally { vi.useRealTimers(); }
  });

  it.each([
    { kind: 'malformed', payload: '{' },
    { kind: 'byte-oversized', payload: 'あ'.repeat(3 * 1024 * 1024) },
  ])('rejects $kind WebSocket responses', async ({ payload }) => {
    const ledger = new BrowserLedger([GOOD], 1, () => new MockSocket((_, sock) => sock.receive(payload)));
    await expect(ledger.tx(PARAMS)).rejects.toThrow(/invalid|oversized/i);
  });

  it('preflights and transports JSON-RPC over CORS without credentials or redirects', async () => {
    const fetcher = vi.fn<typeof fetch>().mockImplementation(async (_, init) => {
      const request = JSON.parse(String(init?.body));
      return Response.json({ id: request.id, result: request.method === 'server_info' ? INFO : { tx: 'data' } });
    });
    const ledger = new BrowserLedger([RPC], 1, undefined, fetcher);
    expect(JSON.parse(await ledger.tx(PARAMS))).toEqual({ tx: 'data' });
    expect(fetcher.mock.calls.map(([url, options]) => [url, JSON.parse(String(options?.body))])).toEqual([
      [RPC, { id: 1, method: 'server_info', params: [{ api_version: 2 }] }],
      [RPC, { id: 2, method: 'tx', params: [JSON.parse(PARAMS)] }],
    ]);
    for (const [, options] of fetcher.mock.calls) {
      expect(options).toMatchObject({ credentials: 'omit', redirect: 'error', referrerPolicy: 'no-referrer' });
    }
  });
});

describe('node transport and settings', () => {
  const photo = 'c'.repeat(64);

  it('sends only the photo ID, expected network, and pagination to a configured node', async () => {
    const fetcher = vi.fn<typeof fetch>().mockResolvedValue(new Response('{}'));
    const node = new BrowserNodes([NODE], fetcher);
    await expect(node.lookup(`${NODE}/other`, photo, 1, 32)).rejects.toThrow('unapproved');
    await expect(node.lookup(NODE, photo.toUpperCase(), 1, 32)).rejects.toThrow('Invalid');
    expect(fetcher).not.toHaveBeenCalled();
    expect(await node.lookup(NODE, photo, 1, 32, 'abc/=')).toBe('{}');
    expect(fetcher).toHaveBeenCalledWith(`${NODE}/v1/photos/${photo}?limit=32&network_id=1&cursor=abc%2F%3D`, expect.objectContaining({
      headers: { accept: 'application/json' }, credentials: 'omit', redirect: 'error', referrerPolicy: 'no-referrer',
    }));
  });

  it('bounds streamed node bodies without using content-length', async () => {
    const stream = new ReadableStream({ start: controller => controller.enqueue(new Uint8Array(256 * 1024 + 1)) });
    const node = new BrowserNodes([NODE], async () => new Response(stream));
    await expect(node.lookup(NODE, photo, 1, 32)).rejects.toThrow('Oversized');
  });

  it('rejects unsafe endpoint URLs and explorer URL substitutions', () => {
    for (const raw of ['http://other.example', 'https://user@other.example', 'https://other.example/?q=1', 'https://other.example/%20']) {
      expect(() => endpoint(raw, 'node')).toThrow();
    }
    expect(endpoint('http://localhost:3010/galaxy/', 'node')).toBe('http://localhost:3010/galaxy');
    expect(() => endpoint('http://[::1]:3010', 'node')).toThrow();
    expect(() => explorerTemplate('https://{hash}.example/tx', '{hash}')).toThrow();
    expect(() => explorerTemplate('javascript:{index}', '{index}')).toThrow();
    expect(validateSettings({ rpcEndpoints: [GOOD, GOOD], nodes: [NODE], discovery: false, explorerTx: '', explorerLedger: '' }))
      .toEqual({ rpcEndpoints: [GOOD], nodes: [NODE], discovery: false, explorerTx: '', explorerLedger: '' });
  });
});
