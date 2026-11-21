import init, { inspect_image, verify, type ImageInspection, type VerificationOptions } from '../../../bindings/wasm/pkg/galaxy_wasm.js';
import { BrowserLedger } from './ledger';
import { BrowserNodes } from './node';

export type Mode =
  | { mode: 'direct' }
  | { mode: 'manual'; manifest_tx: string }
  | { mode: 'lookup'; nodes: string[]; node_lookup_consent: true };

type Photo = { bytes: ArrayBuffer; wasm: string };
export type WorkerRequest =
  | ({ kind: 'inspect' } & Photo)
  | { kind: 'verify'; photo?: Photo; rpcEndpoints: string[]; mode: Mode };

export type WorkerResult =
  | { inspection: ImageInspection }
  | { report: string }
  | { error: string };

const worker = self as unknown as DedicatedWorkerGlobalScope;
let image: Uint8Array | undefined;

async function loadPhoto({ bytes, wasm }: Photo): Promise<Uint8Array> {
  const response = await fetch(wasm, { redirect: 'error', credentials: 'omit', referrerPolicy: 'no-referrer' });
  if (!response.ok) throw new Error('Could not load the local verifier module.');
  await init({ module_or_path: response });
  return new Uint8Array(bytes);
}

worker.addEventListener('message', async ({ data }: MessageEvent<WorkerRequest>) => {
  const respond = (result: WorkerResult) => worker.postMessage(result);
  try {
    if (data.kind === 'inspect') {
      image = await loadPhoto(data);
      respond({ inspection: inspect_image(image) });
      return;
    }
    const bytes = data.photo ? await loadPhoto(data.photo) : image;
    if (!bytes) throw new Error('No photo was loaded.');
    const { rpcEndpoints, mode } = data;
    const options: VerificationOptions = {
      network_config: __DEPLOYMENT_TOML__, rpc_endpoints: rpcEndpoints, ...mode,
    };
    const ledger = new BrowserLedger(rpcEndpoints, __DEPLOYMENT__.networkId);
    try {
      const nodes = mode.mode === 'lookup' && mode.node_lookup_consent === true ? new BrowserNodes(mode.nodes) : undefined;
      respond({ report: await verify(bytes, options, ledger, nodes) });
    } finally {
      ledger.close();
    }
  } catch (error) {
    respond({ error: error instanceof Error ? error.message : 'The local verifier did not complete.' });
  }
});
