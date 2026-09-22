// "Padrões": the preset for AC and for battery, and when an override ends
// (SPEC 13.2). The daemon option is saved through SetDaemonOption.

import type { Control, Defaults, PowerSource, Preset } from '../api';
import { fromPct, toPct } from '../curve-math';
import { CONTROL, POWER, PROFILE_ORDER, profileInfo } from '../labels';
import type { Store } from '../store';
import { $, escape, html, toast } from '../ui';

const SOURCES: PowerSource[] = ['ac', 'battery'];
const CONTROL_LABEL: Record<Control, string> = {
  firmware: `${CONTROL.firmware} (firmware)`,
  fixed: CONTROL.fixed,
  curve: CONTROL.curve,
};

export class DefaultsView {
  private defaults: Defaults | null = null;
  private saved = '';
  private curves: string[] = [];
  private readonly forms: Record<PowerSource, HTMLElement>;

  constructor(private readonly root: HTMLElement, private readonly store: Store) {
    root.append(
      html('<div class="defaults"></div>'),
      html(`
        <div class="defaults-foot">
          <button class="btn btn-primary" data-act="save" disabled>Salvar padrões</button>
          <button class="btn" data-act="copy-ac">Usar o estado atual na tomada</button>
          <button class="btn" data-act="copy-battery">Usar o estado atual na bateria</button>
          <label class="check"><input type="checkbox" data-override-until />
            Ao trocar tomada/bateria, voltar ao padrão</label>
        </div>`),
    );
    const grid = $(root, '.defaults');
    this.forms = { ac: this.form('ac'), battery: this.form('battery') };
    grid.append(this.forms.ac, this.forms.battery);

    root.addEventListener('click', (e) => void this.onAction(e));
    $<HTMLInputElement>(root, '[data-override-until]').addEventListener('change', (e) => {
      const value = (e.target as HTMLInputElement).checked ? 'power-change' : 'manual';
      void this.store.act((api) => api.setOverrideUntil(value), 'Opção salva.');
    });
    store.onState(() => this.syncState());
  }

  async show(): Promise<void> {
    try {
      [this.defaults, this.curves] = await Promise.all([this.store.api.getDefaults(), this.store.api.listCurves()]);
    } catch (e) {
      toast(String(e));
      return;
    }
    this.saved = JSON.stringify(this.defaults);
    for (const s of SOURCES) this.fill(s);
    this.syncState();
  }

  private form(source: PowerSource): HTMLElement {
    const id = (name: string) => `${name}-${source}`;
    const el = html(`
      <article class="card preset" data-source="${source}">
        <h2>${POWER[source]} <span class="now" hidden>agora</span></h2>
        <div class="field">
          <label for="${id('profile')}">Perfil</label>
          <select id="${id('profile')}" data-key="profile"></select>
        </div>
        <fieldset class="field" style="border:0;padding:0;margin:0">
          <legend class="muted small">Controle</legend>
          <div class="radio-row">
            ${(Object.keys(CONTROL_LABEL) as Control[])
              .map((c) => `<label class="check"><input type="radio" name="${id('control')}" value="${c}" /> ${CONTROL_LABEL[c]}</label>`)
              .join('')}
          </div>
        </fieldset>
        <div class="field" data-when="fixed">
          <label for="${id('cpu')}">Boost da CPU <output class="num" data-out="fixedCpu"></output></label>
          <input id="${id('cpu')}" type="range" min="0" max="100" data-key="fixedCpu" />
          <label for="${id('gpu')}">Boost da GPU <output class="num" data-out="fixedGpu"></output></label>
          <input id="${id('gpu')}" type="range" min="0" max="100" data-key="fixedGpu" />
        </div>
        <div class="field" data-when="curve">
          <label for="${id('curve')}">Curva</label>
          <select id="${id('curve')}" data-key="curve"></select>
        </div>
      </article>`);
    el.addEventListener('input', (e) => this.onInput(source, e.target as HTMLInputElement));
    return el;
  }

  private fill(source: PowerSource): void {
    const p = this.defaults?.[source];
    const form = this.forms[source];
    if (!p) return;
    const available = this.store.state?.availableProfiles ?? PROFILE_ORDER;
    const profiles = PROFILE_ORDER.filter((x) => available.includes(x) || x === p.profile);
    $<HTMLSelectElement>(form, '[data-key="profile"]').innerHTML = profiles
      .map((x) => `<option value="${x}" ${x === p.profile ? 'selected' : ''}>${profileInfo(x).name}</option>`)
      .join('');
    const curves = this.curves.includes(p.curve) || !p.curve ? this.curves : [p.curve, ...this.curves];
    $<HTMLSelectElement>(form, '[data-key="curve"]').innerHTML = curves
      .map((c) => `<option value="${escape(c)}" ${c === p.curve ? 'selected' : ''}>${escape(c)}</option>`)
      .join('');
    form.querySelectorAll<HTMLInputElement>('input[type="radio"]').forEach((r) => (r.checked = r.value === p.control));
    for (const key of ['fixedCpu', 'fixedGpu'] as const) {
      const pct = toPct(p[key]);
      const input = $<HTMLInputElement>(form, `[data-key="${key}"]`);
      input.value = String(pct);
      input.style.setProperty('--fill', `${pct}%`);
      $(form, `[data-out="${key}"]`).textContent = `${pct}%`;
    }
    this.showControl(source);
  }

  private showControl(source: PowerSource): void {
    const control = this.defaults?.[source].control;
    this.forms[source].querySelectorAll<HTMLElement>('[data-when]').forEach((el) => (el.hidden = el.dataset.when !== control));
  }

  private onInput(source: PowerSource, target: HTMLInputElement): void {
    const p = this.defaults?.[source];
    if (!p) return;
    if (target.type === 'radio') {
      p.control = target.value as Control;
      this.showControl(source);
    } else if (target.dataset.key === 'fixedCpu' || target.dataset.key === 'fixedGpu') {
      const pct = Number(target.value);
      p[target.dataset.key] = fromPct(pct);
      target.style.setProperty('--fill', `${pct}%`);
      $(this.forms[source], `[data-out="${target.dataset.key}"]`).textContent = `${pct}%`;
    } else if (target.dataset.key === 'profile' || target.dataset.key === 'curve') {
      p[target.dataset.key] = target.value;
    }
    this.syncDirty();
  }

  private syncDirty(): void {
    $<HTMLButtonElement>(this.root, '[data-act="save"]').disabled =
      !this.defaults || JSON.stringify(this.defaults) === this.saved || !this.store.connected;
  }

  private syncState(): void {
    const s = this.store.state;
    for (const source of SOURCES) $(this.forms[source], '.now').hidden = s?.powerSource !== source;
    $<HTMLInputElement>(this.root, '[data-override-until]').checked = s?.overrideUntil !== 'manual';
    this.root.querySelectorAll<HTMLButtonElement>('[data-act^="copy"]').forEach((b) => (b.disabled = !s));
    this.syncDirty();
  }

  private async onAction(e: Event): Promise<void> {
    const button = (e.target as Element).closest<HTMLButtonElement>('[data-act]');
    if (!button || button.disabled) return;
    const act = button.dataset.act;
    if (act === 'save' && this.defaults) {
      const d = this.defaults;
      const ok = await this.store.act(async (api) => {
        for (const s of SOURCES) await api.setDefault(s, d[s] as Partial<Preset>);
      }, 'Padrões salvos.');
      if (ok) await this.show();
    } else if (act === 'copy-ac' || act === 'copy-battery') {
      const target: PowerSource = act === 'copy-ac' ? 'ac' : 'battery';
      const ok = await this.store.act((api) => api.saveAsDefault(target), `Padrão ${POWER[target].toLowerCase()} salvo.`);
      if (ok) await this.show();
    }
  }
}
