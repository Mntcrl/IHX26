import VerifyWorker from './verify.worker?worker&inline';
import wasmAsset from '../../../bindings/wasm/pkg/galaxy_wasm_bg.wasm?url&no-inline';
import { HASH, MAX_IMAGE_BYTES, VERIFY_TIMEOUT_MS } from './constants';
import { renderReport } from './report';
import { installNetworkPolicy, loadSettings, storeSettings, validateSettings } from './settings';
import type { Mode, VerifyRequest, VerifyResult } from './verify.worker';
import './style.css';

const id = <T extends HTMLElement = HTMLElement>(name: string) => document.getElementById(name) as T;
const wasm = new URL(wasmAsset, location.href).href;
const settings = loadSettings();
installNetworkPolicy(settings, wasm);

const form = id<HTMLFormElement>('verify-form');
const modeGroup = id<HTMLFieldSetElement>('mode-group');
const input = id<HTMLInputElement>('photo');
const drop = id('drop-zone');
const manual = id('manual-options');
const manifest = id<HTMLInputElement>('manifest');
const lookup = id('lookup-options');
const consent = id<HTMLInputElement>('consent');
const verify = id<HTMLButtonElement>('verify');
const cancel = id<HTMLButtonElement>('cancel');
const status = id('status');
const results = id('results');
const report = id('report');
const canLookUp = settings.discovery && settings.nodes.length > 0;

let photo: File | undefined;
let active: { worker: Worker; timer: number } | undefined;

id('network').textContent = __DEPLOYMENT__.name.toUpperCase();
id('lookup-notice').textContent = canLookUp
  ? 'Lookup sends the computed photo identifier to the nodes below. They can correlate repeat queries and learn that your browser asked about this photo. This does not send the photo, filename, or sensor data.'
  : settings.discovery ? 'Add at least one Galaxy node in settings to use photo ID lookup.'
  : 'Photo ID lookup is disabled. It can be enabled in settings.';
id('consent-label').hidden = !canLookUp;
if (canLookUp) {
  for (const endpoint of settings.nodes) {
    const li = document.createElement('li');
    li.textContent = endpoint;
    id('lookup-nodes').append(li);
  }
}

function updateForm(): void {
  const mode = form.querySelector<HTMLInputElement>('input[name="mode"]:checked')?.value;
  manual.hidden = mode !== 'manual';
  modeGroup.disabled = !!active;
  manifest.disabled = manual.hidden || !!active;
  consent.disabled = !!active;
  manifest.required = !manual.hidden;
  lookup.hidden = mode !== 'lookup';
  verify.disabled = !photo || !!active || (mode === 'lookup' && (!canLookUp || !consent.checked));
  cancel.hidden = !active;
}

function finish(): void {
  if (active) {
    clearTimeout(active.timer);
    active.worker.terminate();
    active = undefined;
  }
  updateForm();
}

function selectPhoto(file: File | undefined): void {
  finish();
  consent.checked = false;
  results.hidden = true;
  report.replaceChildren();
  if (file && (!file.size || file.size > MAX_IMAGE_BYTES)) {
    status.textContent = file.size ? 'This file exceeds the 16 MiB limit.' : 'This file is empty.';
    photo = undefined;
    input.value = '';
  } else {
    photo = file;
    status.textContent = file ? `Selected locally: ${file.name}` : 'No photo selected.';
  }
  updateForm();
}

input.addEventListener('change', () => selectPhoto(input.files?.[0]));
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
  if (event.dataTransfer.files.length === 1) selectPhoto(event.dataTransfer.files[0]);
  else {
    selectPhoto(undefined);
    status.textContent = 'Drop one photo at a time.';
  }
});

form.addEventListener('change', event => {
  if (event.target instanceof HTMLInputElement && event.target.name === 'mode') consent.checked = false;
  updateForm();
});
cancel.addEventListener('click', () => {
  finish();
  status.textContent = 'Verification cancelled. No verdict was established.';
  (verify.disabled ? consent : verify).focus();
});

form.addEventListener('submit', async event => {
  event.preventDefault();
  if (!photo || active) return;
  const selected = new FormData(form).get('mode');
  let mode: Mode;
  if (selected === 'lookup') {
    if (!canLookUp || !consent.checked) return;
    mode = { mode: 'lookup', nodes: settings.nodes, node_lookup_consent: true };
    consent.checked = false;
  } else if (selected === 'manual') {
    if (!HASH.test(manifest.value)) return;
    mode = { mode: 'manual', manifest_tx: manifest.value };
  } else if (selected === 'direct') mode = { mode: 'direct' };
  else return;

  results.hidden = true;
  report.replaceChildren();
  status.textContent = 'Verifying locally…';
  let worker: Worker | undefined;
  try {
    worker = new VerifyWorker();
    const timer = setTimeout(() => {
      finish();
      status.textContent = 'Verification timed out. No verdict was established.';
    }, VERIFY_TIMEOUT_MS);
    active = { worker, timer };
    updateForm();
    worker.addEventListener('message', ({ data }: MessageEvent<VerifyResult>) => {
      if (active?.worker !== worker) return;
      finish();
      try {
        if ('error' in data) throw new Error(data.error);
        const state = renderReport(report, data.report, settings);
        results.hidden = false;
        status.textContent = `Verification result: ${state}.`;
        id('result-title').focus();
      } catch (error) {
        status.textContent = `Verification could not complete. ${error instanceof Error ? error.message : ''}`;
      }
    }, { once: true });
    worker.addEventListener('error', () => {
      if (active?.worker !== worker) return;
      finish();
      status.textContent = 'The local verifier could not run. No verdict was established.';
    }, { once: true });
    const bytes = await photo.arrayBuffer();
    if (active?.worker !== worker) return; // Cancelled or replaced while reading.
    const request: VerifyRequest = { bytes, wasm, rpcEndpoints: settings.rpcEndpoints, mode };
    worker.postMessage(request, [bytes]);
  } catch {
    if (worker && active?.worker !== worker) return;
    finish();
    status.textContent = 'The photo could not be read or the local verifier could not start.';
  }
});

const rpcEndpoints = id<HTMLTextAreaElement>('rpc-endpoints');
const nodes = id<HTMLTextAreaElement>('node-endpoints');
const discovery = id<HTMLInputElement>('discovery');
const explorerTx = id<HTMLInputElement>('explorer-tx');
const explorerLedger = id<HTMLInputElement>('explorer-ledger');
rpcEndpoints.value = settings.rpcEndpoints.join('\n');
nodes.value = settings.nodes.join('\n');
discovery.checked = settings.discovery;
explorerTx.value = settings.explorerTx;
explorerLedger.value = settings.explorerLedger;
const lines = (text: string) => text.split('\n').map(line => line.trim()).filter(Boolean);
id<HTMLFormElement>('settings-form').addEventListener('submit', event => {
  event.preventDefault();
  try {
    storeSettings(validateSettings({
      rpcEndpoints: lines(rpcEndpoints.value), nodes: lines(nodes.value), discovery: discovery.checked,
      explorerTx: explorerTx.value, explorerLedger: explorerLedger.value,
    }));
    finish();
    location.reload(); // Meta CSP policies only tighten; new settings start a new document.
  } catch (error) {
    id('settings-status').textContent = error instanceof Error ? error.message : 'Could not save settings.';
  }
});
updateForm();
