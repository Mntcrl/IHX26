import VerifyWorker from './verify.worker?worker&inline';
import wasmAsset from '../../../bindings/wasm/pkg/galaxy_wasm_bg.wasm?url&no-inline';
import type { ImageInspection } from '../../../bindings/wasm/pkg/galaxy_wasm.js';
import { HASH, MAX_IMAGE_BYTES, VERIFY_TIMEOUT_MS } from './constants';
import { renderReport } from './report';
import { installNetworkPolicy, loadSettings } from './settings';
import type { Mode, WorkerRequest, WorkerResult } from './verify.worker';
import './style.css';

const id = <T extends HTMLElement = HTMLElement>(name: string) => document.getElementById(name) as T;
const wasm = new URL(wasmAsset, location.href).href;
const settings = loadSettings();
installNetworkPolicy(settings, wasm);

const form = id<HTMLFormElement>('verify-form');
const input = id<HTMLInputElement>('photo');
const drop = id('drop-zone');
const manual = id<HTMLInputElement>('manual');
const manifest = id<HTMLInputElement>('manifest');
const discovery = id<HTMLInputElement>('discovery');
const verify = id<HTMLButtonElement>('verify');
const cancel = id<HTMLButtonElement>('cancel');
const status = id('status');
const results = id('results');
const report = id('report');
const photoId = id('photo-id');
const detection = id('detection');
const card = id('photo-card');
const progress = id('progress');
const copy = id<HTMLButtonElement>('copy');

let photo: File | undefined;
let worker: Worker | undefined;
let inspection: ImageInspection | undefined;
let operation: 'inspect' | 'verify' | undefined;
let timer: number | undefined;

const host = new URL(settings.lookupNode).host;

function updateForm(): void {
  const busy = !!operation;
  const canDiscover = !!inspection?.photo_id && !inspection.xmp && !manual.checked;
  id('lookup-options').hidden = !canDiscover;
  id('lookup-notice').textContent = discovery.checked
    ? `Photo ID is sent to ${host} to find its capture.`
    : 'Lookup off. No photo ID will be sent to a lookup node.';
  discovery.disabled = busy;
  manual.disabled = busy;
  id('manual-options').hidden = !manual.checked;
  manifest.disabled = !manual.checked || busy;
  verify.disabled = !inspection || busy || (manual.checked && !HASH.test(manifest.value.trim()));
  cancel.hidden = operation !== 'verify';
  progress.hidden = operation !== 'verify';
}

function finish(): void {
  clearTimeout(timer);
  worker?.terminate();
  worker = undefined;
  operation = undefined;
  updateForm();
}

function fail(message: string): void {
  finish();
  status.textContent = message;
}

function showInspection(data: ImageInspection): void {
  inspection = data;
  const xmp = data.xmp;
  // The committed capture profile can differ from the container (lossless export).
  const inconsistent = xmp && (xmp.photo_id !== data.photo_id || xmp.network !== __DEPLOYMENT__.name);
  const unreadable = data.xmp_error || inconsistent || data.errors.some(error => /XMP/i.test(error.detail));

  detection.dataset.kind = unreadable ? 'warning' : xmp ? 'embedded' : 'regular';
  detection.textContent = unreadable
    ? 'PHØTON metadata present but unreadable/inconsistent'
    : xmp ? 'PHØTON photo — XRPL references embedded'
      : data.container ? 'Regular photo — no PHØTON metadata' : 'Photo could not be inspected';
  id('photo-identity').hidden = !data.photo_id;
  photoId.textContent = data.photo_id ?? '';
  id('file-name').textContent = photo?.name ?? '';
  copy.textContent = 'Copy ID';
  card.hidden = false;
  status.textContent = data.errors[0]?.detail ?? '';
}

function startWorker(): Worker {
  const instance = new VerifyWorker();
  worker = instance;
  instance.addEventListener('message', ({ data }: MessageEvent<WorkerResult>) => {
    if (worker !== instance) return;
    if ('error' in data) {
      fail(`The local verifier could not complete. ${data.error}`);
      return;
    }
    if ('inspection' in data) {
      clearTimeout(timer);
      operation = undefined;
      showInspection(data.inspection);
      updateForm();
      return;
    }
    finish();
    try {
      renderReport(report, data.report, settings);
      results.hidden = false;
      status.textContent = '';
      id('result-title').focus({ preventScroll: true });
      results.scrollIntoView({ block: 'nearest', behavior: 'auto' });
    } catch (error) {
      status.textContent = `Could not render the report. ${error instanceof Error ? error.message : ''}`;
    }
  });
  instance.addEventListener('error', () => {
    if (worker === instance) fail('The local verifier could not run. No verdict was established.');
  });
  return instance;
}

async function selectPhoto(file?: File): Promise<void> {
  photo = undefined;
  inspection = undefined;
  finish();
  results.hidden = true;
  report.replaceChildren();
  card.hidden = true;
  photoId.textContent = '';
  if (!file || !file.size || file.size > MAX_IMAGE_BYTES) {
    status.textContent = !file ? '' : file.size ? 'This file exceeds the 16 MiB limit.' : 'This file is empty.';
    input.value = '';
    return;
  }
  photo = file;
  operation = 'inspect';
  status.textContent = 'Reading photo locally…';
  updateForm();
  let instance: Worker | undefined;
  try {
    instance = startWorker();
    timer = setTimeout(() => fail('Reading the photo timed out.'), VERIFY_TIMEOUT_MS);
    const bytes = await file.arrayBuffer();
    if (worker === instance) {
      const request: WorkerRequest = { kind: 'inspect', bytes, wasm };
      instance.postMessage(request, [bytes]);
    }
  } catch {
    if (worker === instance) fail('The photo could not be read or the local verifier could not start.');
  }
}

input.addEventListener('change', () => void selectPhoto(input.files?.[0]));
document.addEventListener('dragover', event => {
  if (!event.dataTransfer?.types.includes('Files')) return;
  event.preventDefault();
  const inside = event.target instanceof Node && drop.contains(event.target);
  event.dataTransfer.dropEffect = inside ? 'copy' : 'none';
  drop.classList.toggle('dragging', inside);
});
drop.addEventListener('dragleave', event => {
  if (!(event.relatedTarget instanceof Node) || !drop.contains(event.relatedTarget)) drop.classList.remove('dragging');
});
document.addEventListener('drop', event => {
  if (!event.dataTransfer?.types.includes('Files')) return;
  event.preventDefault();
  drop.classList.remove('dragging');
  if (!(event.target instanceof Node) || !drop.contains(event.target)) return;
  input.value = '';
  if (event.dataTransfer.files.length === 1) void selectPhoto(event.dataTransfer.files[0]);
  else {
    void selectPhoto();
    status.textContent = 'Drop one photo at a time.';
  }
});

form.addEventListener('change', updateForm);
manifest.addEventListener('input', updateForm);
cancel.addEventListener('click', () => {
  fail('Verification cancelled. No verdict was established.');
  verify.focus();
});
copy.addEventListener('click', async () => {
  const text = photoId.textContent;
  if (!text) return;
  try {
    await navigator.clipboard.writeText(text);
    if (text === photoId.textContent) copy.textContent = 'Copied';
  } catch {
    if (card.hidden || text !== photoId.textContent) return;
    const range = document.createRange();
    range.selectNodeContents(photoId);
    const selection = window.getSelection();
    selection?.removeAllRanges();
    selection?.addRange(range);
    status.textContent = 'Photo ID selected. Use your keyboard or browser menu to copy it.';
  }
});

form.addEventListener('submit', async event => {
  event.preventDefault();
  if (!photo || !inspection || operation) return;
  let mode: Mode = { mode: 'direct' };
  if (manual.checked) {
    const hash = manifest.value.trim();
    if (!HASH.test(hash)) return;
    mode = { mode: 'manual', manifest_tx: hash };
  } else if (!inspection.xmp && inspection.photo_id && discovery.checked) {
    mode = { mode: 'lookup', nodes: [settings.lookupNode], node_lookup_consent: true };
  }

  results.hidden = true;
  report.replaceChildren();
  operation = 'verify';
  status.textContent = mode.mode === 'lookup' ? `Looking up this photo on ${host}…` : 'Verifying photo…';
  updateForm();
  let instance: Worker | undefined;
  try {
    const reuse = !!worker;
    instance = worker ?? startWorker();
    timer = setTimeout(() => fail('Verification timed out. No verdict was established.'), VERIFY_TIMEOUT_MS);
    const request: WorkerRequest = { kind: 'verify', rpcEndpoints: settings.rpcEndpoints, mode };
    if (reuse) instance.postMessage(request);
    else {
      const bytes = await photo.arrayBuffer();
      if (worker !== instance) return;
      request.photo = { bytes, wasm };
      instance.postMessage(request, [bytes]);
    }
  } catch {
    if (worker === instance) fail('The photo could not be read or the local verifier could not start.');
  }
});

updateForm();
