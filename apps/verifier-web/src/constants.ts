export const MAX_IMAGE_BYTES = 16 * 1024 * 1024; // galaxy-image::MAX_IMAGE_BYTES
export const MAX_ENDPOINTS = 8;
export const HASH = /^[a-fA-F0-9]{64}$/;
export const VERIFY_TIMEOUT_MS = 180_000;

export interface Deployment {
  name: string;
  networkId: number;
  endpoints: string[];
  lookupNode: string;
  explorerTx: string;
  explorerLedger: string;
}

declare global {
  const __DEPLOYMENT__: Deployment;
  const __DEPLOYMENT_TOML__: string;
}
