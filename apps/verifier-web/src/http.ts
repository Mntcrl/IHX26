export type Fetch = typeof fetch;
export const REQUEST_TIMEOUT_MS = 12_000;
export const MAX_RPC_BYTES = 8 * 1024 * 1024;

export function object(value: unknown): Record<string, unknown> {
  if (typeof value !== 'object' || value === null || Array.isArray(value)) throw new Error('Expected a JSON object');
  return value as Record<string, unknown>;
}

export function u32(value: unknown): value is number {
  return typeof value === 'number' && Number.isInteger(value) && value >= 0 && value <= 0xffff_ffff;
}

export async function fetchJson(
  fetcher: Fetch, url: string, init: RequestInit, cap: number, timeout: number,
): Promise<Record<string, unknown>> {
  const controller = new AbortController();
  const timer = setTimeout(() => controller.abort(), timeout);
  try {
    const response = await fetcher(url, {
      ...init, signal: controller.signal, redirect: 'error', credentials: 'omit',
      cache: 'no-store', referrerPolicy: 'no-referrer',
    });
    const reader = response.body?.getReader();
    try {
      if (!response.ok) throw new Error(`Network request returned HTTP ${response.status}`);
      if (!reader || Number(response.headers.get('content-length')) > cap) throw new Error('Missing or oversized response');
      const decoder = new TextDecoder('utf-8', { fatal: true });
      const text: string[] = [];
      let size = 0;
      for (;;) {
        const { done, value } = await reader.read();
        if (done) break;
        size += value.byteLength;
        if (size > cap) throw new Error('Oversized network response');
        text.push(decoder.decode(value, { stream: true }));
      }
      text.push(decoder.decode());
      return object(JSON.parse(text.join('')));
    } finally {
      // Abort without awaiting a potentially unresponsive source's cancel().
      if (reader) void reader.cancel().catch(() => {});
    }
  } finally {
    clearTimeout(timer);
    controller.abort();
  }
}
