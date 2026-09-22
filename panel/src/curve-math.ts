// Curve rules shared by the editor and the simulated daemon. They mirror
// alienfan-core (SPEC 6.2 and 6.3); the daemon validates again on save.

import type { Point } from './api';

export const TEMP_MIN = 0;
export const TEMP_MAX = 105;
export const MIN_POINTS = 2;
export const MAX_POINTS = 16;

/** Flat outside the points, linear between them, rounded. */
export function interpolate(points: Point[], temp: number): number {
  if (points.length === 0) return 0;
  const first = points[0];
  const last = points[points.length - 1];
  if (temp <= first[0]) return first[1];
  if (temp >= last[0]) return last[1];
  for (let i = 1; i < points.length; i++) {
    const [t1, b1] = points[i];
    if (temp <= t1) {
      const [t0, b0] = points[i - 1];
      return Math.round(b0 + ((temp - t0) / (t1 - t0)) * (b1 - b0));
    }
  }
  return last[1];
}

/**
 * Where point `i` may go: between its neighbours in temperature (strictly),
 * and not below the previous boost nor above the next one.
 */
export function bounds(points: Point[], i: number) {
  const prev = points[i - 1];
  const next = points[i + 1];
  return {
    tMin: prev ? prev[0] + 1 : TEMP_MIN,
    tMax: next ? next[0] - 1 : TEMP_MAX,
    bMin: prev ? prev[1] : 0,
    bMax: next ? next[1] : 255,
  };
}

export const clamp = (v: number, lo: number, hi: number) => Math.min(hi, Math.max(lo, v));

/** Moves point `i`, keeping every rule. Returns a new array. */
export function movePoint(points: Point[], i: number, temp: number, boost: number): Point[] {
  const b = bounds(points, i);
  const out = points.map((p) => [...p] as Point);
  out[i] = [clamp(Math.round(temp), b.tMin, b.tMax), clamp(Math.round(boost), b.bMin, b.bMax)];
  return out;
}

/** Adds a point on the line at `temp`, if there is room. */
export function addPoint(points: Point[], temp: number): Point[] {
  const t = Math.round(temp);
  if (points.length >= MAX_POINTS || points.some(([pt]) => pt === t)) return points;
  const out = [...points.map((p) => [...p] as Point), [t, interpolate(points, t)] as Point];
  return out.sort((a, b) => a[0] - b[0]);
}

export function removePoint(points: Point[], i: number): Point[] {
  if (points.length <= MIN_POINTS) return points;
  return points.filter((_, j) => j !== i);
}

export const toPct = (boost: number) => Math.round((boost * 100) / 255);
export const fromPct = (pct: number) => Math.round((clamp(pct, 0, 100) * 255) / 100);
