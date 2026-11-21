import './scrollcraft/scrollcraft.js';

// Presentation only. No photo, report, settings or verification state is read here.
type ScrollScene = { layout(): void; destroy(): void };
const { ScrollCraft } = window as typeof window & {
  ScrollCraft: { mount(): ScrollScene };
};
const sampling = document.querySelector<HTMLElement>('#sampling')!;
const stage = sampling.querySelector<HTMLElement>('[data-sc-stage]')!;
const figure = sampling.querySelector<HTMLElement>('figure')!;
const tiles = figure.querySelectorAll<SVGUseElement>('#sample-tiles use');
const field = document.querySelector<HTMLElement>('.workbench')!;
const cinematic = matchMedia('(min-width: 48rem) and (min-height: 42rem) and (prefers-reduced-motion: no-preference)');

// The source cells are illustrative SVG coordinates. The target is a 4 × 4
// contact sheet at (406, 56), and each cell is 28 SVG units square.
tiles.forEach((tile, i) => {
  tile.style.setProperty('--dx', `${406 + (i % 4) * 28 - tile.x.baseVal.value}px`);
  tile.style.setProperty('--dy', `${56 + Math.floor(i / 4) * 28 - tile.y.baseVal.value}px`);
  tile.style.setProperty('--delay', (i * .01).toFixed(2));
});

let scene: ScrollScene;
function mount(): void {
  scene?.destroy();
  sampling.classList.remove('sc-act--pinned');
  stage.classList.remove('sc-stage');
  sampling.style.removeProperty('height');
  sampling.dataset.scAct = cinematic.matches ? 'pin' : 'flow';
  scene = ScrollCraft.mount();
}
mount();
cinematic.addEventListener('change', mount);

// File selection, settings and expanded results can move the story down the
// document. Refresh the engine's cached section positions without remounting.
new ResizeObserver(() => scene.layout()).observe(field);

// The shoot harness samples painted displacement, not raw scroll progress.
const leading = tiles[0];
const trailing = tiles[tiles.length - 1];
const finalOffset = parseFloat(trailing.style.getPropertyValue('--dx'));
new MutationObserver(() => {
  const first = new DOMMatrixReadOnly(getComputedStyle(leading).transform);
  const last = new DOMMatrixReadOnly(getComputedStyle(trailing).transform);
  figure.dataset.scVerifyState = `${first.e.toFixed(1)},${first.f.toFixed(1)}:${last.e.toFixed(1)},${last.f.toFixed(1)}`;
  if (Math.abs(last.e - finalOffset) < .1) figure.dataset.scVerifyHold = 'true';
  else delete figure.dataset.scVerifyHold;
}).observe(sampling, { attributes: true, attributeFilter: ['style'] });
