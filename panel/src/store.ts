// Shared state of the panel: what the daemon last reported.

import type { Backend, DaemonState, Telemetry } from './api';
import { toast } from './ui';

type Listener = () => void;

export class Store {
  state: DaemonState | null = null;
  telemetry: Telemetry | null = null;
  private stateListeners = new Set<Listener>();
  private telemetryListeners = new Set<Listener>();

  constructor(readonly api: Backend) {
    api.onState((s) => {
      this.state = s;
      this.emitState();
    });
    api.onTelemetry((t) => {
      this.telemetry = t;
      this.telemetryListeners.forEach((l) => l());
    });
    api.onDaemon((connected) => {
      if (connected) {
        void this.refresh();
      } else {
        this.state = null;
        this.emitState();
      }
    });
  }

  get connected(): boolean {
    return this.state !== null;
  }

  async refresh(): Promise<void> {
    try {
      this.state = await this.api.getState();
      if (this.state) this.telemetry = await this.api.getTelemetry();
    } catch (e) {
      this.state = null;
      toast(String(e));
    }
    this.emitState();
    this.telemetryListeners.forEach((l) => l());
  }

  onState(l: Listener): void {
    this.stateListeners.add(l);
  }

  onTelemetry(l: Listener): void {
    this.telemetryListeners.add(l);
  }

  /** Runs a daemon call; failures show the daemon's message. */
  async act(f: (api: Backend) => Promise<void>, done?: string): Promise<boolean> {
    try {
      await f(this.api);
      if (done) toast(done, 'info');
      return true;
    } catch (e) {
      toast(String(e));
      return false;
    } finally {
      await this.refresh();
    }
  }

  private emitState(): void {
    this.stateListeners.forEach((l) => l());
  }
}
