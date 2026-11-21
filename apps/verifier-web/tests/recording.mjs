import { isDeepStrictEqual } from 'node:util';

// Same first-match semantics as galaxy-verifier::recorded, ignoring api_version.
export function replay(exchanges, method, { api_version, ...params }) {
  const entry = exchanges.find(({ method: name, params: { api_version: _, ...recorded } }) => (
    method === name && isDeepStrictEqual(params, recorded)
  ));
  if (!entry) throw new Error(`no recorded answer for ${method}`);
  return entry.result;
}
