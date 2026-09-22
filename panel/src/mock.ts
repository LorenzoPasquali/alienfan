// A simulated daemon for `npm run dev` in a plain browser, so the UI can be
// worked on without Tauri or hardware. RPM figures come from the Phase 0
// measurements (docs/HARDWARE.md). `?mock=offline|emergency|no-permission`
// starts it in that state.

import type {
  Backend,
  Control,
  Curve,
  DaemonState,
  Defaults,
  FanId,
  FanTelemetry,
  OverrideUntil,
  Point,
  Preset,
  Target,
  Telemetry,
} from './api';
import { interpolate } from './curve-math';

/** Idle RPM with boost 0, per profile (run 1, rounded). */
const BASE_RPM: Record<string, number> = {
  'quiet': 0,
  'balanced': 1750,
  'balanced-performance': 2450,
  'custom': 2900,
  'cool': 4300,
  'performance': 6200,
};
const TOP_RPM = 6600;

const SHIPPED_CURVES: Record<string, Curve> = {
  silencioso: curve([[50, 0], [70, 38], [80, 102], [90, 204], [95, 255]], 3, 40, 10),
  equilibrado: curve([[45, 0], [60, 51], [70, 115], [80, 179], [90, 255]], 3, 40, 10),
  agressivo: curve([[40, 26], [55, 102], [65, 166], [75, 217], [85, 255]], 2, 80, 15),
};

function curve(points: Point[], hysteresisC: number, up: number, down: number): Curve {
  return { cpu: points, gpu: points.map((p) => [...p] as Point), hysteresisC, rampUpPerS: up, rampDownPerS: down };
}

const clone = <T>(v: T): T => structuredClone(v);

export class MockBackend implements Backend {
  private running: boolean;
  private health: DaemonState['health'] = 'ok';
  private power: DaemonState['powerSource'] = 'ac';
  private overrideUntil: OverrideUntil = 'power-change';
  private override: Preset | null = null;
  private defaults: Defaults = {
    ac: { profile: 'balanced-performance', control: 'firmware', fixedCpu: 0, fixedGpu: 0, curve: 'equilibrado' },
    battery: { profile: 'balanced', control: 'firmware', fixedCpu: 0, fixedGpu: 0, curve: 'silencioso' },
  };
  private curves: Record<string, Curve> = clone(SHIPPED_CURVES);
  private temps = { cpu: 52, gpu: 41 };
  private rpm = { cpu: 2400, gpu: 2500 };
  private boost = { cpu: 0, gpu: 0 };
  private telemetryCbs: ((t: Telemetry) => void)[] = [];
  private stateCbs: ((s: DaemonState) => void)[] = [];
  private daemonCbs: ((c: boolean) => void)[] = [];

  constructor() {
    const mode = new URLSearchParams(location.search).get('mock');
    this.running = mode !== 'offline';
    if (mode === 'emergency') this.temps.cpu = 97;
    if (mode === 'no-permission') this.health = 'no-permission';
    setInterval(() => this.tick(), 1000);
  }

  private effective(): Preset {
    return this.override ?? this.defaults[this.power];
  }

  private state(): DaemonState {
    const p = this.effective();
    const emergency = this.temps.cpu > 95 || this.temps.gpu > 95;
    const health = emergency ? 'emergency' : this.health;
    const messages: Record<string, string> = {
      'emergency': 'Temperatura acima de 95 °C: ventoinhas no máximo.',
      'no-permission': 'Sem permissão de escrita no perfil e no boost. Rode `alienfan doctor`.',
    };
    return {
      version: '0.1.0 (simulado)',
      health,
      healthMessage: messages[health] ?? '',
      powerSource: this.power,
      profile: p.profile,
      availableProfiles: ['cool', 'quiet', 'balanced', 'balanced-performance', 'performance', 'custom'],
      control: p.control,
      activeCurve: p.control === 'curve' ? p.curve : '',
      overrideActive: this.override !== null,
      boostRequiresCustom: false,
      overrideUntil: this.overrideUntil,
      emergencyTempC: 95,
    };
  }

  private tick(): void {
    if (!this.running) return;
    const drift = (t: number) => Math.min(99, Math.max(35, t + (Math.random() - 0.48) * 1.6));
    this.temps = { cpu: drift(this.temps.cpu), gpu: drift(this.temps.gpu) };
    const p = this.effective();
    const emergency = this.state().health === 'emergency';
    for (const fan of ['cpu', 'gpu'] as FanId[]) {
      let target = 0;
      if (emergency) target = 255;
      else if (p.control === 'fixed') target = fan === 'cpu' ? p.fixedCpu : p.fixedGpu;
      else if (p.control === 'curve') target = interpolate(this.curves[p.curve]?.[fan] ?? [], this.temps[fan]);
      this.boost[fan] += Math.max(-10, Math.min(40, target - this.boost[fan]));
      const base = BASE_RPM[p.profile] ?? 2000;
      const lift = 1 - (1 - this.boost[fan] / 255) ** 2.5;
      const goal = base + (TOP_RPM - base) * lift;
      this.rpm[fan] += (goal - this.rpm[fan]) * 0.35;
    }
    const t = this.telemetry();
    this.telemetryCbs.forEach((cb) => cb(t));
    this.emitState();
  }

  private telemetry(): Telemetry {
    const fan = (id: FanId): FanTelemetry => ({
      id,
      label: id === 'cpu' ? 'CPU Fan' : 'GPU Fan',
      rpm: Math.round(this.rpm[id]),
      rpmMax: 6000,
      boost: Math.round(this.boost[id]),
      targetBoost: Math.round(this.boost[id]),
      tempC: Math.round(this.temps[id]),
      sensor: `alienware_wmi:${id.toUpperCase()}`,
    });
    return {
      fans: [fan('cpu'), fan('gpu')],
      temps: { 'alienware_wmi:CPU': this.temps.cpu, 'alienware_wmi:GPU': this.temps.gpu, 'coretemp:Package id 0': this.temps.cpu + 3 },
    };
  }

  private emitState(): void {
    const s = this.state();
    this.stateCbs.forEach((cb) => cb(s));
  }

  private async change(f: () => void): Promise<void> {
    if (!this.running) throw 'o daemon não está rodando';
    f();
    this.emitState();
  }

  private setOverride(patch: Partial<Preset>): Promise<void> {
    return this.change(() => {
      this.override = { ...this.effective(), ...patch };
    });
  }

  async getState() { return this.running ? this.state() : null; }
  async getTelemetry() { return this.telemetry(); }
  async getDefaults() { return clone(this.defaults); }
  async listCurves() { return Object.keys(this.curves).sort(); }

  async getCurve(name: string) {
    const c = this.curves[name];
    if (!c) throw `curva "${name}" não existe`;
    return clone(c);
  }

  setProfile(profile: string) { return this.setOverride({ profile }); }

  setFixedBoost(fan: FanId | 'all', boost: number) {
    const p = this.effective();
    let cpu = p.control === 'fixed' ? p.fixedCpu : Math.round(this.boost.cpu);
    let gpu = p.control === 'fixed' ? p.fixedGpu : Math.round(this.boost.gpu);
    if (fan !== 'gpu') cpu = boost;
    if (fan !== 'cpu') gpu = boost;
    return this.setOverride({ control: 'fixed', fixedCpu: cpu, fixedGpu: gpu });
  }

  setControl(control: Control, curve = '') {
    const p = this.effective();
    if (control === 'curve') {
      const name = curve || this.defaults[this.power].curve;
      if (!this.curves[name]) return Promise.reject(`curva "${name}" não existe`);
      return this.setOverride({ control, curve: name });
    }
    if (control === 'fixed' && p.control !== 'fixed') {
      return this.setOverride({ control, fixedCpu: Math.round(this.boost.cpu), fixedGpu: Math.round(this.boost.gpu) });
    }
    return this.setOverride({ control });
  }

  restoreDefault() { return this.change(() => { this.override = null; }); }

  saveAsDefault(target: Target) {
    return this.change(() => {
      const targets: PowerSource[] = target === 'both' ? ['ac', 'battery'] : [target];
      for (const t of targets) this.defaults[t] = { ...this.defaults[t], ...this.effective() };
      if (targets.includes(this.power)) this.override = null;
    });
  }

  setDefault(target: Target, preset: Partial<Preset>) {
    return this.change(() => {
      const targets: PowerSource[] = target === 'both' ? ['ac', 'battery'] : [target];
      for (const t of targets) this.defaults[t] = { ...this.defaults[t], ...preset };
    });
  }

  saveCurve(name: string, c: Curve) {
    if (!/^[\p{L}\p{N}_-]{1,32}$/u.test(name)) return Promise.reject(`nome de curva inválido: "${name}"`);
    return this.change(() => { this.curves[name] = clone(c); });
  }

  deleteCurve(name: string) {
    const used = (['ac', 'battery'] as PowerSource[]).some((s) => this.defaults[s].control === 'curve' && this.defaults[s].curve === name)
      || (this.override?.control === 'curve' && this.override.curve === name);
    if (used) return Promise.reject(`a curva "${name}" está em uso`);
    return this.change(() => { delete this.curves[name]; });
  }

  setOverrideUntil(value: OverrideUntil) { return this.change(() => { this.overrideUntil = value; }); }

  async startDaemon() {
    this.running = true;
    this.daemonCbs.forEach((cb) => cb(true));
    this.emitState();
  }

  onTelemetry(cb: (t: Telemetry) => void) { this.telemetryCbs.push(cb); }
  onState(cb: (s: DaemonState) => void) { this.stateCbs.push(cb); }
  onDaemon(cb: (c: boolean) => void) { this.daemonCbs.push(cb); }
}

type PowerSource = DaemonState['powerSource'];
