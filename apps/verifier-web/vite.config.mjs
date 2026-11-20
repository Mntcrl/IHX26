import { readFileSync } from 'node:fs';
import { parse } from 'smol-toml';
import { defineConfig } from 'vite';

const configPath = process.env.GALAXY_WEB_NETWORK_CONFIG
  ?? new URL('../../deployments/networks/xrpl-testnet.toml', import.meta.url);

export default defineConfig(({ mode }) => {
  // Unit tests isolate the transports and don't need a deployment manifest.
  const toml = mode === 'test' ? '' : readFileSync(configPath, 'utf8');
  const config = toml && parse(toml);
  if (config && (config.name !== 'xrpl-testnet' || config.network_id !== 1)) {
    throw new Error('The browser verifier build requires an XRPL Testnet config');
  }
  const deployment = config && {
    name: config.name,
    networkId: config.network_id,
    endpoints: config.websocket?.length ? config.websocket : config.json_rpc,
    explorerTx: config.explorer_tx,
    explorerLedger: config.explorer_ledger,
  };
  return {
    base: './',
    // The inline Blob worker inherits the app document's exact connect-src CSP.
    worker: { format: 'es' },
    define: {
      __DEPLOYMENT_TOML__: JSON.stringify(toml),
      __DEPLOYMENT__: JSON.stringify(deployment),
    },
  };
});
