import init, { verify, type VerificationOptions } from '../../../bindings/wasm/pkg/galaxy_wasm.js';
import { BrowserLedger } from './ledger';
import { BrowserNodes } from './node';

export type Mode =
  | { mode: 'direct' }
  | { mode: 'manual'; manifest_tx: string }
  | { mode: 'lookup'; nodes: string[]; node_lookup_consent: true };

export interface VerifyRequest {
  bytes: ArrayBuffer;
  wasm: string;
  rpcEndpoints: string[];
  mode: Mode;
}
export type VerifyResult = { report: string } | { error: string };

const worker = self as unknown as DedicatedWorkerGlobalScope;
worker.addEventListener('message', async ({ data }: MessageEvent<VerifyRequest>) => {
  const { bytes, wasm, rpcEndpoints, mode } = data;
  const respond = (result: VerifyResult) => worker.postMessage(result);
  try {
    const module = await fetch(wasm, { redirect: 'error', credentials: 'omit', referrerPolicy: 'no-referrer' });
    if (!module.ok) throw new Error('Could not load the local verifier module.');
    await init({ module_or_path: module });
    const options: VerificationOptions = {
      network_config: __DEPLOYMENT_TOML__, rpc_endpoints: rpcEndpoints, ...mode,
    };
    const ledger = new BrowserLedger(rpcEndpoints, __DEPLOYMENT__.networkId);
    try {
      const nodes = mode.mode === 'lookup' && mode.node_lookup_consent === true ? new BrowserNodes(mode.nodes) : undefined;
      respond({ report: await verify(new Uint8Array(bytes), options, ledger, nodes) });
    } finally {
      ledger.close();
    }
  } catch (error) {
    respond({ error: error instanceof Error ? error.message : 'The local verifier did not complete.' });
  }
}, { once: true });
