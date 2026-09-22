// SVG editor for a temperature → boost curve (SPEC 13.2, "Curvas").
// X is 20–100 °C, Y is 0–100% boost; points keep the SPEC 6.2 rules while
// they move.

import type { FanId, Point } from './api';
import { addPoint, bounds, clamp, fromPct, movePoint, removePoint, toPct } from './curve-math';

const SVG = 'http://www.w3.org/2000/svg';
const W = 640;
const H = 300;
const PAD = { left: 44, right: 14, top: 12, bottom: 28 };
const T_MIN = 20;
const T_MAX = 100;
const FANS: FanId[] = ['cpu', 'gpu'];
const FAN_NAME = { cpu: 'CPU', gpu: 'GPU' };

const x = (t: number) => PAD.left + ((clamp(t, T_MIN, T_MAX) - T_MIN) / (T_MAX - T_MIN)) * (W - PAD.left - PAD.right);
const y = (boost: number) => PAD.top + (1 - boost / 255) * (H - PAD.top - PAD.bottom);
const tAt = (px: number) => T_MIN + ((px - PAD.left) / (W - PAD.left - PAD.right)) * (T_MAX - T_MIN);
const boostAt = (py: number) => (1 - (py - PAD.top) / (H - PAD.top - PAD.bottom)) * 255;

function node<K extends keyof SVGElementTagNameMap>(tag: K, attrs: Record<string, string | number> = {}) {
  const n = document.createElementNS(SVG, tag);
  for (const [k, v] of Object.entries(attrs)) n.setAttribute(k, String(v));
  return n;
}

export interface Live {
  tempC: number | null;
  boost: number;
}

export class CurveEditor {
  readonly svg: SVGSVGElement;
  private readonly danger: SVGRectElement;
  private readonly dangerText: SVGTextElement;
  private readonly lines: Record<FanId, SVGPathElement>;
  private readonly liveLayer: SVGGElement;
  private readonly pointLayer: SVGGElement;
  private points: Record<FanId, Point[]> = { cpu: [], gpu: [] };
  private active: FanId = 'cpu';
  private linked = true;
  private dragging = -1;

  constructor(private readonly onChange: (fan: FanId, points: Point[]) => void) {
    this.svg = node('svg', {
      viewBox: `0 0 ${W} ${H}`,
      class: 'chart',
      role: 'group',
      'aria-label': 'Gráfico da curva: temperatura no eixo horizontal, boost no vertical',
    });
    const defs = node('defs');
    const hatch = node('pattern', { id: 'hatch', width: 8, height: 8, patternUnits: 'userSpaceOnUse', patternTransform: 'rotate(45)' });
    hatch.append(node('line', { x1: 0, y1: 0, x2: 0, y2: 8, class: 'hatch-line' }));
    defs.append(hatch);
    this.svg.append(defs);

    for (let t = T_MIN; t <= T_MAX; t += 10) {
      this.svg.append(node('line', { x1: x(t), x2: x(t), y1: PAD.top, y2: H - PAD.bottom, class: 'grid-line' }));
      const label = node('text', { x: x(t), y: H - 8, 'text-anchor': 'middle', class: 'axis-text' });
      label.textContent = `${t}°`;
      this.svg.append(label);
    }
    for (const pct of [0, 25, 50, 75, 100]) {
      const py = y(fromPct(pct));
      this.svg.append(node('line', { x1: PAD.left, x2: W - PAD.right, y1: py, y2: py, class: 'grid-line' }));
      const label = node('text', { x: PAD.left - 8, y: py + 4, 'text-anchor': 'end', class: 'axis-text' });
      label.textContent = `${pct}%`;
      this.svg.append(label);
    }

    this.danger = node('rect', { y: PAD.top, height: H - PAD.top - PAD.bottom, class: 'danger' });
    this.dangerText = node('text', { y: H - PAD.bottom - 8, 'text-anchor': 'end', class: 'danger-text' });
    this.dangerText.textContent = 'emergência';
    this.lines = { gpu: node('path', { class: 'line gpu' }), cpu: node('path', { class: 'line cpu' }) };
    this.liveLayer = node('g');
    this.pointLayer = node('g');
    this.svg.append(this.danger, this.dangerText, this.lines.gpu, this.lines.cpu, this.liveLayer, this.pointLayer);

    this.svg.addEventListener('pointermove', (e) => this.onDrag(e));
    this.svg.addEventListener('pointerup', (e) => this.endDrag(e));
    this.svg.addEventListener('pointercancel', (e) => this.endDrag(e));
    this.svg.addEventListener('dblclick', (e) => {
      if ((e.target as Element).classList.contains('point')) return;
      const [px] = this.toSvg(e);
      this.change(addPoint(this.points[this.active], clamp(tAt(px), T_MIN, T_MAX)));
    });
  }

  /** `linked`: one line drives both fans. */
  set(points: Record<FanId, Point[]>, linked: boolean, active: FanId): void {
    this.points = points;
    this.linked = linked;
    this.active = linked ? 'cpu' : active;
    this.render();
  }

  setEmergency(tempC: number): void {
    const x0 = x(tempC);
    this.danger.setAttribute('x', String(x0));
    this.danger.setAttribute('width', String(Math.max(0, W - PAD.right - x0)));
    // Left of the band, which is often too narrow for the label.
    this.dangerText.setAttribute('x', String(x0 - 6));
  }

  setLive(live: Partial<Record<FanId, Live>>): void {
    this.liveLayer.replaceChildren();
    for (const fan of FANS) {
      const l = live[fan];
      if (!l || l.tempC === null || l.tempC < T_MIN || l.tempC > T_MAX) continue;
      const g = node('g', { class: `live-marker ${fan}` });
      g.append(
        node('line', { x1: x(l.tempC), x2: x(l.tempC), y1: PAD.top, y2: H - PAD.bottom, class: 'live' }),
        node('circle', { cx: x(l.tempC), cy: y(l.boost), r: 4, class: 'live-dot' }),
      );
      this.liveLayer.append(g);
    }
  }

  private change(points: Point[]): void {
    this.onChange(this.active, points);
  }

  private render(): void {
    for (const fan of FANS) {
      const pts = this.points[fan];
      if (pts.length === 0) {
        this.lines[fan].removeAttribute('d');
        continue;
      }
      // Flat outside the points, as the daemon interpolates.
      const path = [[T_MIN, pts[0][1]], ...pts, [T_MAX, pts[pts.length - 1][1]]]
        .map(([t, b], i) => `${i ? 'L' : 'M'}${x(t).toFixed(1)} ${y(b).toFixed(1)}`)
        .join(' ');
      this.lines[fan].setAttribute('d', path);
      this.lines[fan].classList.toggle('is-dim', !this.linked && fan !== this.active);
    }
    // The active line on top.
    this.svg.insertBefore(this.lines[this.active], this.liveLayer);

    const focused = this.pointLayer.querySelector(':focus') as SVGElement | null;
    const focusIndex = focused ? Number(focused.dataset.index) : -1;
    this.pointLayer.replaceChildren();
    const pts = this.points[this.active];
    const who = this.linked ? 'CPU e GPU' : FAN_NAME[this.active];
    pts.forEach(([t, b], i) => {
      const c = node('circle', {
        cx: x(t),
        cy: y(b),
        r: 7,
        class: `point ${this.active}`,
        tabindex: 0,
        role: 'slider',
        'aria-label': `${who}, ponto ${i + 1} de ${pts.length}`,
        'aria-valuetext': `${t} °C, ${toPct(b)}%`,
        'aria-valuemin': 0,
        'aria-valuemax': 100,
        'aria-valuenow': toPct(b),
      });
      c.dataset.index = String(i);
      c.addEventListener('pointerdown', (e) => this.startDrag(e, i));
      c.addEventListener('contextmenu', (e) => {
        e.preventDefault();
        this.change(removePoint(this.points[this.active], i));
      });
      c.addEventListener('keydown', (e) => this.onKey(e, i));
      this.pointLayer.append(c);
    });
    if (focusIndex >= 0) {
      const again = this.pointLayer.children[Math.min(focusIndex, pts.length - 1)] as SVGElement | undefined;
      again?.focus();
    }
  }

  private toSvg(e: MouseEvent): [number, number] {
    const m = this.svg.getScreenCTM();
    if (!m) return [0, 0];
    const p = new DOMPoint(e.clientX, e.clientY).matrixTransform(m.inverse());
    return [p.x, p.y];
  }

  private startDrag(e: PointerEvent, i: number): void {
    if (e.button !== 0) return;
    e.preventDefault();
    this.dragging = i;
    // Capture on the SVG: point circles are recreated on every change.
    this.svg.setPointerCapture(e.pointerId);
    (e.target as SVGElement).focus();
  }

  private onDrag(e: PointerEvent): void {
    if (this.dragging < 0) return;
    const [px, py] = this.toSvg(e);
    const b = bounds(this.points[this.active], this.dragging);
    const t = clamp(tAt(px), Math.max(T_MIN, b.tMin), Math.min(T_MAX, b.tMax));
    this.change(movePoint(this.points[this.active], this.dragging, t, boostAt(py)));
  }

  private endDrag(e: PointerEvent): void {
    if (this.dragging < 0) return;
    this.dragging = -1;
    if (this.svg.hasPointerCapture(e.pointerId)) this.svg.releasePointerCapture(e.pointerId);
  }

  private onKey(e: KeyboardEvent, i: number): void {
    const pts = this.points[this.active];
    const [t, b] = pts[i];
    const step = e.shiftKey ? 5 : 1;
    let next: Point | null = null;
    if (e.key === 'ArrowLeft') next = [t - step, b];
    else if (e.key === 'ArrowRight') next = [t + step, b];
    else if (e.key === 'ArrowUp') next = [t, fromPct(toPct(b) + step)];
    else if (e.key === 'ArrowDown') next = [t, fromPct(toPct(b) - step)];
    else if (e.key === 'Delete' || e.key === 'Backspace') {
      e.preventDefault();
      this.change(removePoint(pts, i));
      return;
    } else return;
    e.preventDefault();
    this.change(movePoint(pts, i, next[0], next[1]));
  }
}
