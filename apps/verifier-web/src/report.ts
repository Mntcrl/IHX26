import { HASH } from './constants';
import type { Settings } from './settings';

const STATES = {
  VERIFIED: 'The complete accepted cryptographic verification policy is satisfied.',
  PENDING: 'A compatible capture exists, but its positive proof is not yet available within the permitted window.',
  INVALID: 'A presented image binding, transaction, signature, or proof failed verification.',
  INCONCLUSIVE: 'No accepted positive physical consistency proof exists, or the available evidence is insufficient.',
  NOT_FOUND: 'No matching registration was found in the consulted history.',
  UNAVAILABLE: 'The relevant network or historical data is unavailable.',
  UNSUPPORTED: 'The format, policy, circuit, or attestation environment is not supported.',
};

type State = keyof typeof STATES;
const STATUS = { pass: 'Passed', fail: 'Failed', unavailable: 'Unavailable', not_applicable: 'Not a pass/fail check', skipped: 'Not evaluated' };
const BASIS = {
  cryptographic: 'Cryptographic check', 'zk-circuit': 'Proven by circuit when passed',
  'committed-only': 'Committed only · not proven', 'ledger-rpc': 'XRPL RPC check', attestation: 'Attestation check',
};

interface Check {
  id: string;
  status: keyof typeof STATUS;
  proven_by: keyof typeof BASIS;
  detail: string;
}

interface Candidate {
  manifest_tx: string;
  proof_tx: string | null;
  state: State;
  summary: string;
  proofs?: Candidate[];
}

interface Report {
  version: 'galaxy-report/v1';
  state: State;
  summary: string;
  network: string;
  mode: string;
  image: { format: string; profile: string; photo_id: string; rgb_root: string; xmp: unknown } | null;
  capture: {
    manifest_tx: string; L0: number; Lpre: number; capture_id: string; account: string;
    registration_tx: string; claimed_time_ms: number; ledger_close_time: number;
    attestation: string | null; app_id: string | null; counter: number | null;
  } | null;
  proof: {
    proof_tx: string | null; Lsel: number | null; Lproof: number | null; deadline: number | null;
    selection_hash: string | null; circuit_id: string; policy_id: string;
    policy_name: string | null; ceremony: string | null;
  } | null;
  checks: Check[];
  candidates: Candidate[];
  limitations: string[];
  trust: {
    rpc_endpoints: string[]; rpc_trust: string; lookup_endpoints: string[];
    ledger_ranges: { purpose: string; min: number; max: number }[]; latest_validated: number | null;
  };
}

const GROUPS: [string, string[]][] = [
  ['Image integrity', ['image.']],
  ['RGB commitment', ['capture.rgb_binding']],
  ['Device attestation', ['registration.attestation', 'capture.assertion', 'capture.counter', 'registration.revocation']],
  ['Manifest integrity', ['refs.metadata', 'capture.decode', 'capture.network', 'capture.photo_binding', 'capture.account_binding', 'registration.resolve', 'registration.bundle']],
  ['Ledger anchoring', ['ledger.network', 'capture.found', 'capture.validated_success', 'capture.unique', 'ledger.preledger_window']],
  ['Challenge binding', ['ledger.selection', 'ledger.challenge']],
  ['Circuit recognition', ['circuit.']],
  ['ZK proof validity', ['proof.']],
  ['Sensor quality policy', ['policy.sensor_quality']],
  ['LiDAR geometry policy', ['policy.lidar_geometry']],
  ['RGB–LiDAR correspondence policy', ['policy.rgb_lidar']],
  ['Temporal consistency policy', ['policy.temporal']],
  ['IMU consistency policy', ['policy.motion_imu']],
  ['Bluetooth aggregates', ['evidence.bluetooth']],
  ['Noise statistics', ['evidence.noise']],
];

function element<K extends keyof HTMLElementTagNameMap>(tag: K, text?: string, className?: string): HTMLElementTagNameMap[K] {
  const result = document.createElement(tag);
  if (text !== undefined) result.textContent = text;
  if (className !== undefined) result.className = className;
  return result;
}

function link(template: string, key: '{hash}' | '{index}', value: string | number | null | undefined): HTMLElement | string | null | undefined {
  const valid = key === '{hash}' ? typeof value === 'string' && HASH.test(value) : Number.isInteger(value);
  if (!template || !valid) return value == null ? value : String(value);
  const label = String(value);
  const a = element('a', label);
  a.href = template.replace(key, encodeURIComponent(label));
  a.target = '_blank';
  a.rel = 'noopener noreferrer';
  return a;
}

function utc(ms: number | undefined): string | undefined {
  if (ms === undefined) return undefined;
  const date = new Date(ms);
  return Number.isFinite(date.getTime()) ? date.toISOString() : 'Outside the supported display range';
}

function checksTable(checks: Check[]): HTMLElement {
  const table = element('table');
  table.createCaption().textContent = 'Verification properties';
  const columns = table.createTHead().insertRow();
  for (const title of ['Property', 'Checks and evidence']) {
    const heading = element('th', title);
    heading.scope = 'col';
    columns.append(heading);
  }
  const body = table.createTBody();
  for (const [label, keys] of GROUPS) {
    const row = body.insertRow();
    const heading = element('th', label);
    heading.scope = 'row';
    row.append(heading);
    const cell = row.insertCell();
    const list = element('ul', undefined, 'checks');
    for (const check of checks.filter(item => keys.some(key => key.endsWith('.') ? item.id.startsWith(key) : item.id === key))) {
      const item = element('li');
      const result = element('strong', STATUS[check.status] ?? check.status, 'check-status');
      result.dataset.status = check.status;
      const basis = element('span', BASIS[check.proven_by] ?? check.proven_by, 'basis');
      basis.dataset.basis = check.proven_by;
      item.append(result, basis, element('div', check.detail), element('code', check.id));
      list.append(item);
    }
    cell.append(list);
  }
  return table;
}

function candidatesList(candidates: Candidate[], template: string): HTMLElement {
  const list = element('ul', undefined, 'candidates');
  for (const candidate of candidates) {
    const row = element('li');
    const tx = candidate.proof_tx ?? candidate.manifest_tx;
    row.append(element('strong', `${candidate.state}: `), link(template, '{hash}', tx) ?? tx, element('p', candidate.summary));
    if (candidate.proofs?.length) row.append(candidatesList(candidate.proofs, template));
    list.append(row);
  }
  return list;
}

export function renderReport(target: HTMLElement, json: string, { explorerTx, explorerLedger }: Settings): State {
  const report: Report = JSON.parse(json);
  if (report.version !== 'galaxy-report/v1' || !Object.hasOwn(STATES, report.state)) {
    throw new Error('The verifier returned an unknown report format.');
  }
  const badge = element('span', report.state, 'state');
  badge.dataset.state = report.state;
  const headline = element('p', report.summary, 'headline');
  const description = element('p', STATES[report.state]);
  const metadata = element('dl', undefined, 'metadata');
  const { image, capture, proof, trust } = report;
  const fields: [string, Node | string | number | null | undefined][] = [
    ['XRPL network', report.network], ['Lookup mode', report.mode],
    ['Image container / profile', image && `${image.format} / ${image.profile}`],
    ['Photo identifier', image?.photo_id], ['RGB commitment', image?.rgb_root],
    ['Galaxy XMP', image && (image.xmp ? 'Present (untrusted lookup hints)' : 'Not present or unreadable')],
    ['Capture identifier', capture?.capture_id], ['Capture account', capture?.account],
    ['Manifest transaction', link(explorerTx, '{hash}', capture?.manifest_tx)],
    ['Proof transaction', link(explorerTx, '{hash}', proof?.proof_tx)],
    ['Device registration transaction', link(explorerTx, '{hash}', capture?.registration_tx)],
    ['Attestation environment', capture?.attestation], ['Attestation app ID', capture?.app_id],
    ['Lpre · prior ledger', link(explorerLedger, '{index}', capture?.Lpre)],
    ['L0 · manifest ledger', link(explorerLedger, '{index}', capture?.L0)],
    ['Lsel · selection ledger', link(explorerLedger, '{index}', proof?.Lsel)],
    ['Lproof · proof ledger', link(explorerLedger, '{index}', proof?.Lproof)],
    ['Proof deadline ledger', link(explorerLedger, '{index}', proof?.deadline)],
    ['Last validated ledger', link(explorerLedger, '{index}', trust.latest_validated)],
    ['Policy identifier', proof?.policy_id], ['Policy name', proof?.policy_name],
    ['Circuit identifier', proof?.circuit_id], ['Circuit ceremony', proof?.ceremony],
    ['Claimed capture time (UTC)', utc(capture?.claimed_time_ms)],
    ['Manifest ledger close time (UTC)', utc(capture ? capture.ledger_close_time * 1000 : undefined)],
  ];
  for (const [label, value] of fields) {
    const dd = element('dd');
    dd.append(value instanceof Node ? value : value == null ? 'Not established' : String(value));
    metadata.append(element('dt', label), dd);
  }

  const sourceDetails = element('details');
  sourceDetails.append(element('summary', 'Network sources and consulted ledger ranges'));
  sourceDetails.append(element('p', `Configured XRPL endpoints (${trust.rpc_trust} RPC trust): ${trust.rpc_endpoints.join(', ') || 'none'}`));
  sourceDetails.append(element('p', `Consulted Galaxy nodes: ${trust.lookup_endpoints.join(', ') || 'none'}`));
  if (trust.ledger_ranges.length) {
    const ranges = element('ul');
    for (const { purpose, min, max } of trust.ledger_ranges) ranges.append(element('li', `${purpose}: ${min}–${max}`));
    sourceDetails.append(ranges);
  } else sourceDetails.append(element('p', 'No ledger ranges were consulted.'));

  const limitations = element('ul');
  for (const limitation of report.limitations) limitations.append(element('li', limitation));
  const raw = element('details');
  raw.append(element('summary', 'Report JSON'), element('pre', JSON.stringify(report, null, 2)));
  target.replaceChildren(badge, headline, description, element('h3', 'Capture and ledger details'), metadata, checksTable(report.checks));
  if (report.candidates.length) {
    const candidates = element('details');
    candidates.append(element('summary', 'Candidate records'), candidatesList(report.candidates, explorerTx));
    target.append(candidates);
  }
  target.append(sourceDetails, element('h3', 'Limitations'), limitations, raw);
  return report.state;
}
