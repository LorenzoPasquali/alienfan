// The animated fan of a card (SPEC 13.3). Rotation runs in JS with
// requestAnimationFrame: changing a CSS animation's duration makes it jump.

const SVG = 'http://www.w3.org/2000/svg';
const BLADES = 7;
/** One swept blade pointing up; the hub has radius 24, the frame 93. */
const BLADE =
  'M 0 -24 C 2 -50 12 -72 29.1 -79.9 A 85 85 0 0 1 69.6 -48.8 ' +
  'C 52 -42 28 -26 14.1 -19.4 A 24 24 0 0 0 0 -24 Z';
/** Real RPM is divided down: 6000 rpm on screen would only be a blur. */
const RPM_PER_TURN_PER_S = 450;
const MAX_TURNS_PER_S = 7;
const INERTIA_S = 0.6;
const BLUR_FROM_TURNS = 3;

const reducedMotion = matchMedia('(prefers-reduced-motion: reduce)');
const fans = new Set<FanView>();

document.addEventListener('visibilitychange', () => {
  fans.forEach((f) => (document.hidden ? f.pause() : f.resume()));
});

function el<K extends keyof SVGElementTagNameMap>(tag: K, attrs: Record<string, string> = {}) {
  const node = document.createElementNS(SVG, tag);
  for (const [k, v] of Object.entries(attrs)) node.setAttribute(k, v);
  return node;
}

function bladeGroup(className: string): SVGGElement {
  const g = el('g', { class: className });
  for (let i = 0; i < BLADES; i++) {
    g.append(el('path', { d: BLADE, transform: `rotate(${(i * 360) / BLADES})` }));
  }
  return g;
}

export class FanView {
  readonly svg: SVGSVGElement;
  private readonly blades: SVGGElement;
  private readonly blur: SVGGElement;
  private rpm = 0;
  private turnsPerS = 0;
  private angle = 0;
  private last = 0;
  private frame = 0;

  constructor(id: string) {
    this.svg = el('svg', { viewBox: '-100 -100 200 200', class: 'fan', 'aria-hidden': 'true' });
    const glowId = `fan-glow-${id}`;
    const defs = el('defs');
    const gradient = el('radialGradient', { id: glowId });
    gradient.append(
      el('stop', { offset: '0', class: 'fan-glow-center' }),
      el('stop', { offset: '1', class: 'fan-glow-edge' }),
    );
    defs.append(gradient);
    this.blur = bladeGroup('fan-blur');
    this.blades = bladeGroup('fan-blades');
    this.svg.append(
      defs,
      el('circle', { r: '92', class: 'fan-glow', fill: `url(#${glowId})` }),
      el('circle', { r: '93', class: 'fan-frame' }),
      this.blur,
      this.blades,
      el('circle', { r: '24', class: 'fan-hub' }),
      el('circle', { r: '7', class: 'fan-cap' }),
    );
    fans.add(this);
    this.resume();
  }

  /** `rpmMax` scales the glow; the real top speed can exceed it. */
  setRpm(rpm: number, rpmMax: number): void {
    this.rpm = rpm;
    const level = rpmMax > 0 ? Math.min(1, rpm / rpmMax) : 0;
    this.svg.style.setProperty('--glow', level.toFixed(3));
  }

  pause(): void {
    cancelAnimationFrame(this.frame);
    this.frame = 0;
  }

  resume(): void {
    if (this.frame || document.hidden) return;
    this.last = 0;
    this.frame = requestAnimationFrame(this.tick);
  }

  destroy(): void {
    this.pause();
    fans.delete(this);
  }

  private tick = (now: number): void => {
    const dt = this.last ? Math.min(0.1, (now - this.last) / 1000) : 0;
    this.last = now;
    const target = Math.min(MAX_TURNS_PER_S, this.rpm / RPM_PER_TURN_PER_S);
    // First-order lag: spins up and down smoothly instead of jumping.
    this.turnsPerS += (target - this.turnsPerS) * (1 - Math.exp(-dt / INERTIA_S));

    if (!reducedMotion.matches) {
      this.angle = (this.angle + 360 * this.turnsPerS * dt) % 360;
      this.blades.setAttribute('transform', `rotate(${this.angle.toFixed(2)})`);
      const blur = Math.min(1, Math.max(0, (this.turnsPerS - BLUR_FROM_TURNS) / (MAX_TURNS_PER_S - BLUR_FROM_TURNS)));
      this.blur.style.opacity = (blur * 0.55).toFixed(3);
      this.blur.setAttribute('transform', `rotate(${(this.angle - this.turnsPerS * 5).toFixed(2)})`);
    } else {
      this.blur.style.opacity = '0';
    }
    this.frame = requestAnimationFrame(this.tick);
  };
}
