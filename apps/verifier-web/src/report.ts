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
  ['Image', ['image.', 'refs.metadata', 'capture.photo_binding', 'capture.rgb_binding']],
  ['Capture', ['capture.found', 'capture.validated_success', 'capture.decode', 'capture.network', 'capture.account_binding']],
  ['Device attestation', ['registration.', 'capture.assertion', 'capture.counter']],
  ['Ledger', ['ledger.', 'history.', 'capture.txn_chain', 'capture.unique']],
  ['Proof', ['circuit.', 'proof.']],
  ['Policy', ['policy.', 'evidence.']],
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

function checkGroups(checks: Check[]): HTMLElement {
  const groups = element('section', undefined, 'check-groups');
  groups.append(element('h3', 'Verification checks'));
  const remaining = new Set(checks);
  for (const [label, keys] of GROUPS) {
    const items = [...remaining].filter(item => keys.some(key => key.endsWith('.') ? item.id.startsWith(key) : item.id === key));
    for (const item of items) remaining.delete(item);
    if (label === 'Policy') items.push(...remaining); // Surface newly introduced checks until classified.
    const group = element('details', undefined, 'check-group');
    const summary = element('summary', label);
    const passed = items.filter(check => check.status === 'pass').length;
    const failed = items.filter(check => check.status === 'fail').length;
    const count = element('span', failed ? `${failed} failed · ${passed} passed` : passed ? `${passed} passed` : 'No checks passed', 'check-count');
    if (failed) count.dataset.status = 'fail';
    summary.append(count);
    const list = element('ul', undefined, 'checks');
    for (const check of items) {
      const item = element('li');
      const result = element('strong', STATUS[check.status] ?? check.status, 'check-status');
      result.dataset.status = check.status;
      const basis = element('span', BASIS[check.proven_by] ?? check.proven_by, 'basis');
      basis.dataset.basis = check.proven_by;
      item.append(result, basis, element('div', check.detail), element('code', check.id));
      list.append(item);
    }
    group.append(summary, list);
    groups.append(group);
  }
  return groups;
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
  const verdict = element('div', undefined, 'verdict');
  const badge = element('div', report.state, 'state');
  verdict.dataset.state = report.state;
  const headline = element('p', report.summary, 'headline');
  const description = element('p', STATES[report.state]);
  const metadata = element('dl', undefined, 'metadata');
  const { image, capture, proof, trust } = report;
  verdict.append(badge, headline, description);
  if (report.mode === 'lookup' && (report.state === 'NOT_FOUND' || report.state === 'UNAVAILABLE')) {
    const ranges = trust.ledger_ranges.filter(range => range.purpose === 'lookup index (untrusted)');
    verdict.append(element('p', ranges.length
      ? `Node-reported index coverage: ${ranges.map(({ min, max }) => `${min}–${max}`).join(', ')}.`
      : 'Node index coverage could not be established.', 'coverage'));
  }
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
    ['Circuit identifier', proof?.circuit_id],
    ['Circuit ceremony', proof?.ceremony === 'development-single-party' ? 'Development setup (Testnet) · single party' : proof?.ceremony],
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
  sourceDetails.append(element('p', `Consulted lookup nodes: ${trust.lookup_endpoints.join(', ') || 'none'}`));
  if (trust.ledger_ranges.length) {
    const ranges = element('ul');
    for (const { purpose, min, max } of trust.ledger_ranges) ranges.append(element('li', `${purpose}: ${min}–${max}`));
    sourceDetails.append(ranges);
  } else sourceDetails.append(element('p', 'No ledger ranges were consulted.'));

  const limitations = element('ul');
  for (const limitation of report.limitations) limitations.append(element('li', limitation));
  const raw = element('details');
  raw.append(element('summary', 'Report JSON'), element('pre', JSON.stringify(report, null, 2)));
  const record = element('details');
  record.open = !!capture;
  record.append(element('summary', 'Capture and ledger details'), metadata);
  target.replaceChildren(verdict, checkGroups(report.checks), record);
  if (report.candidates.length) {
    const candidates = element('details');
    candidates.append(element('summary', 'Candidate records'), candidatesList(report.candidates, explorerTx));
    target.append(candidates);
  }
  target.append(sourceDetails, element('h3', 'Limitations'), limitations, raw);
  return report.state;
}
