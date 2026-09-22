// UI names (pt-BR). Keys are the daemon's values.

import type { Control, Health, PowerSource } from './api';

export interface ProfileInfo {
  name: string;
  hint: string;
  icon: string;
  badge?: string;
}

/** 16×16 stroke icons, drawn with currentColor. */
const ICONS = {
  moon: '<path d="M12.5 10.5A5.5 5.5 0 0 1 5.5 3.5a5.5 5.5 0 1 0 7 7z"/>',
  scale: '<path d="M8 2.5v11M4 13.5h8M3 5h10M3 5l-1.5 4a2 2 0 0 0 3 0zM13 5l-1.5 4a2 2 0 0 0 3 0z"/>',
  gauge: '<path d="M2.5 11a5.5 5.5 0 1 1 11 0M8 11l3-4"/>',
  snow: '<path d="M8 1.5v13M2.4 4.75l11.2 6.5M2.4 11.25l11.2-6.5M6 2.8l2 1.6 2-1.6M6 13.2l2-1.6 2 1.6"/>',
  bolt: '<path d="M9 1.5 3.5 9H8l-1 5.5L12.5 7H8z"/>',
  sliders: '<path d="M3 2.5v11M8 2.5v11M13 2.5v11M1.5 10h3M6.5 5h3M11.5 8h3"/>',
};

export const icon = (name: keyof typeof ICONS) =>
  `<svg viewBox="0 0 16 16" class="icon" aria-hidden="true" fill="none" stroke="currentColor" stroke-width="1.4" stroke-linecap="round" stroke-linejoin="round">${ICONS[name]}</svg>`;

/** Hints follow the Phase 0 measurements (docs/HARDWARE.md). */
export const PROFILES: Record<string, ProfileInfo> = {
  'quiet': { name: 'Silencioso', hint: 'Ventoinhas paradas em repouso.', icon: 'moon' },
  'balanced': { name: 'Equilibrado', hint: 'Base baixa; o firmware acelera com a carga.', icon: 'scale' },
  'balanced-performance': { name: 'Equilibrado+', hint: 'Um pouco mais de ar e de desempenho.', icon: 'gauge' },
  'cool': { name: 'Frio', hint: 'Prioriza temperatura baixa: bem mais ruído.', icon: 'snow' },
  'performance': { name: 'Desempenho', hint: 'G-Mode: ventoinhas no alto o tempo todo.', icon: 'bolt', badge: 'G-Mode' },
  'custom': { name: 'Personalizado', hint: 'Perfil do firmware com base mais alta.', icon: 'sliders' },
};

/** From quietest to loudest, as measured; `custom` last. */
export const PROFILE_ORDER = ['quiet', 'balanced', 'balanced-performance', 'cool', 'performance', 'custom'];

export const profileInfo = (p: string): ProfileInfo =>
  PROFILES[p] ?? { name: p, hint: '', icon: 'sliders' };

export const POWER: Record<PowerSource, string> = { ac: 'Na tomada', battery: 'Na bateria' };

export const HEALTH: Record<Health, string> = {
  'ok': 'Tudo certo',
  'degraded': 'Degradado',
  'emergency': 'Emergência',
  'no-permission': 'Sem permissão',
  'no-driver': 'Sem driver',
};

export const CONTROL: Record<Control, string> = {
  firmware: 'Automático',
  fixed: 'Manual',
  curve: 'Curva',
};

export const FAN_NAME = { cpu: 'CPU', gpu: 'GPU' } as const;

/** Temperature band that colors a card (SPEC 13.4). */
export function band(tempC: number | null): 'cool' | 'warm' | 'hot' {
  if (tempC === null || tempC < 60) return 'cool';
  return tempC <= 80 ? 'warm' : 'hot';
}
