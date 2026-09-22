// What the panel needs from alienfand. In Tauri the Rust side forwards it
// over D-Bus; in a plain browser (vite dev) a simulated daemon answers.

export type Health = 'ok' | 'degraded' | 'emergency' | 'no-permission' | 'no-driver';
export type Control = 'firmware' | 'fixed' | 'curve';
export type PowerSource = 'ac' | 'battery';
export type FanId = 'cpu' | 'gpu';
export type Target = PowerSource | 'both';
export type OverrideUntil = 'power-change' | 'manual';

export interface DaemonState {
  version: string;
  health: Health;
  healthMessage: string;
  powerSource: PowerSource;
  profile: string;
  availableProfiles: string[];
  control: Control;
  activeCurve: string;
  overrideActive: boolean;
  boostRequiresCustom: boolean;
  overrideUntil: OverrideUntil;
  emergencyTempC: number;
}

export interface FanTelemetry {
  id: FanId;
  label: string;
  rpm: number;
  rpmMax: number;
  boost: number;
  targetBoost: number;
  tempC: number | null;
  sensor: string;
}

export interface Telemetry {
  fans: FanTelemetry[];
  temps: Record<string, number>;
}

export interface Preset {
  profile: string;
  control: Control;
  fixedCpu: number;
  fixedGpu: number;
  curve: string;
}

export interface Defaults {
  ac: Preset;
  battery: Preset;
}

/** `[temp °C, boost 0–255]`, temperatures strictly increasing. */
export type Point = [number, number];

export interface Curve {
  cpu: Point[];
  gpu: Point[];
  hysteresisC: number;
  rampUpPerS: number;
  rampDownPerS: number;
}

export interface Backend {
  /** `null` while the daemon is not on the bus. */
  getState(): Promise<DaemonState | null>;
  getTelemetry(): Promise<Telemetry>;
  getDefaults(): Promise<Defaults>;
  listCurves(): Promise<string[]>;
  getCurve(name: string): Promise<Curve>;

  setProfile(profile: string): Promise<void>;
  setFixedBoost(fan: FanId | 'all', boost: number): Promise<void>;
  setControl(control: Control, curve?: string): Promise<void>;
  restoreDefault(): Promise<void>;
  saveAsDefault(target: Target): Promise<void>;
  setDefault(target: Target, preset: Partial<Preset>): Promise<void>;
  saveCurve(name: string, curve: Curve): Promise<void>;
  deleteCurve(name: string): Promise<void>;
  setOverrideUntil(value: OverrideUntil): Promise<void>;
  startDaemon(): Promise<void>;

  onTelemetry(cb: (t: Telemetry) => void): void;
  onState(cb: (s: DaemonState) => void): void;
  onDaemon(cb: (connected: boolean) => void): void;
}

export async function connect(): Promise<Backend> {
  if ('__TAURI_INTERNALS__' in window) {
    const { TauriBackend } = await import('./tauri');
    return new TauriBackend();
  }
  const { MockBackend } = await import('./mock');
  return new MockBackend();
}
