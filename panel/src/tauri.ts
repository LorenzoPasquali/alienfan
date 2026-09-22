// Backend inside Tauri: commands and events of src-tauri/src/lib.rs.

import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';

import type {
  Backend,
  Control,
  Curve,
  DaemonState,
  Defaults,
  FanId,
  OverrideUntil,
  Preset,
  Target,
  Telemetry,
} from './api';

export class TauriBackend implements Backend {
  getState(): Promise<DaemonState | null> {
    return invoke('get_state');
  }

  getTelemetry(): Promise<Telemetry> {
    return invoke('get_telemetry');
  }

  getDefaults(): Promise<Defaults> {
    return invoke('get_defaults');
  }

  listCurves(): Promise<string[]> {
    return invoke('list_curves');
  }

  getCurve(name: string): Promise<Curve> {
    return invoke('get_curve', { name });
  }

  setProfile(profile: string): Promise<void> {
    return invoke('set_profile', { profile });
  }

  setFixedBoost(fan: FanId | 'all', boost: number): Promise<void> {
    return invoke('set_fixed_boost', { fan, boost });
  }

  setControl(control: Control, curve = ''): Promise<void> {
    return invoke('set_control', { control, curve });
  }

  restoreDefault(): Promise<void> {
    return invoke('restore_default');
  }

  saveAsDefault(target: Target): Promise<void> {
    return invoke('save_as_default', { target });
  }

  setDefault(target: Target, preset: Partial<Preset>): Promise<void> {
    return invoke('set_default', { target, preset });
  }

  saveCurve(name: string, curve: Curve): Promise<void> {
    return invoke('save_curve', { name, curve });
  }

  deleteCurve(name: string): Promise<void> {
    return invoke('delete_curve', { name });
  }

  setOverrideUntil(value: OverrideUntil): Promise<void> {
    return invoke('set_override_until', { value });
  }

  startDaemon(): Promise<void> {
    return invoke('start_daemon');
  }

  onTelemetry(cb: (t: Telemetry) => void): void {
    void listen<Telemetry>('telemetry', (e) => cb(e.payload));
  }

  onState(cb: (s: DaemonState) => void): void {
    void listen<DaemonState>('state', (e) => cb(e.payload));
  }

  onDaemon(cb: (connected: boolean) => void): void {
    void listen<{ connected: boolean }>('daemon', (e) => cb(e.payload.connected));
  }
}
