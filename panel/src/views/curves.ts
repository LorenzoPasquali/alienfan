// "Curvas": list, create, duplicate, delete, and edit curves (SPEC 13.2).

import type { Curve, Defaults, FanId, Point } from '../api';
import { CurveEditor } from '../curve-editor';
import type { Store } from '../store';
import { $, escape, html, toast } from '../ui';

/** Starting points (SPEC 7.2). */
const PRESETS: Record<string, Curve> = {
  Silencioso: preset([[50, 0], [70, 38], [80, 102], [90, 204], [95, 255]], 3, 40, 10),
  Equilibrado: preset([[45, 0], [60, 51], [70, 115], [80, 179], [90, 255]], 3, 40, 10),
  Agressivo: preset([[40, 26], [55, 102], [65, 166], [75, 217], [85, 255]], 2, 80, 15),
};

function preset(points: Point[], hysteresisC: number, up: number, down: number): Curve {
  return { cpu: points, gpu: points, hysteresisC, rampUpPerS: up, rampDownPerS: down };
}

const copy = (c: Curve): Curve => structuredClone(c);
const same = (a: Point[], b: Point[]) => JSON.stringify(a) === JSON.stringify(b);

export class CurvesView {
  private readonly list: HTMLElement;
  private readonly editor: CurveEditor;
  private readonly nameInput: HTMLInputElement;
  private readonly title: HTMLElement;
  private names: string[] = [];
  private defaults: Defaults | null = null;
  private selected: string | null = null;
  private curve: Curve | null = null;
  private saved: string | null = null;
  private isNew = false;
  private linked = true;
  private active: FanId = 'cpu';
  private confirmDelete = 0;

  constructor(private readonly root: HTMLElement, private readonly store: Store) {
    root.append(
      html(`
        <div class="curves">
          <aside class="card curve-list" aria-label="Curvas salvas">
            <ul role="list"></ul>
            <div class="row">
              <button class="btn" data-act="new">Nova</button>
              <button class="btn" data-act="duplicate">Duplicar</button>
              <button class="btn" data-act="delete">Apagar</button>
            </div>
            <div class="row small muted">Começar de:</div>
            <div class="row" data-presets></div>
          </aside>
          <div class="card editor">
            <div class="editor-head">
              <h2></h2>
              <input type="text" class="name-input" aria-label="Nome da nova curva"
                placeholder="nome-da-curva" maxlength="32" hidden />
              <div class="legend"><span class="cpu">CPU</span><span class="gpu">GPU</span></div>
              <label class="check small"><input type="checkbox" data-linked /> Mesma curva para CPU e GPU</label>
              <div class="fan-switch" role="group" aria-label="Ventoinha em edição" hidden>
                <button data-fan="cpu" aria-pressed="true">CPU</button>
                <button data-fan="gpu" aria-pressed="false">GPU</button>
              </div>
            </div>
            <div data-chart></div>
            <p class="small muted">Arraste os pontos. Duplo clique adiciona; botão direito ou Delete remove. Setas movem o ponto em foco (Shift: ×5).</p>
            <div class="fields">
              <div class="field">
                <label for="hysteresis">Histerese (°C)</label>
                <input id="hysteresis" type="number" min="0" max="15" step="0.5" data-field="hysteresisC" />
                <p>Quanto a temperatura precisa cair antes de a ventoinha desacelerar.</p>
              </div>
              <div class="field">
                <label for="ramp-up">Subida (por segundo)</label>
                <input id="ramp-up" type="number" min="1" max="255" step="1" data-field="rampUpPerS" />
                <p>Quanto o boost pode subir a cada segundo, em unidades de 0 a 255.</p>
              </div>
              <div class="field">
                <label for="ramp-down">Descida (por segundo)</label>
                <input id="ramp-down" type="number" min="1" max="255" step="1" data-field="rampDownPerS" />
                <p>Descer devagar evita que a ventoinha fique subindo e descendo.</p>
              </div>
            </div>
            <div class="editor-actions">
              <button class="btn btn-primary" data-act="save">Salvar</button>
              <button class="btn" data-act="test">Testar agora</button>
              <span class="muted" data-dirty hidden>Alterações não salvas</span>
            </div>
          </div>
        </div>`),
    );
    this.list = $(root, 'ul');
    this.title = $(root, '.editor-head h2');
    this.nameInput = $(root, '.name-input');
    this.editor = new CurveEditor((fan, points) => this.onPoints(fan, points));
    $(root, '[data-chart]').append(this.editor.svg);

    const presets = $(root, '[data-presets]');
    for (const name of Object.keys(PRESETS)) {
      const b = html(`<button class="btn">${name}</button>`);
      b.addEventListener('click', () => this.startFrom(PRESETS[name]));
      presets.append(b);
    }
    root.addEventListener('click', (e) => this.onAction(e));
    $<HTMLInputElement>(root, '[data-linked]').addEventListener('change', (e) => {
      this.linked = (e.target as HTMLInputElement).checked;
      if (this.linked && this.curve) this.curve.gpu = structuredClone(this.curve.cpu);
      this.renderEditor();
    });
    root.querySelectorAll<HTMLButtonElement>('.fan-switch button').forEach((b) =>
      b.addEventListener('click', () => {
        this.active = b.dataset.fan as FanId;
        this.renderEditor();
      }),
    );
    root.querySelectorAll<HTMLInputElement>('[data-field]').forEach((input) =>
      input.addEventListener('input', () => {
        if (!this.curve) return;
        const key = input.dataset.field as 'hysteresisC' | 'rampUpPerS' | 'rampDownPerS';
        this.curve[key] = Number(input.value);
        this.renderDirty();
      }),
    );
    this.nameInput.addEventListener('input', () => this.renderDirty());

    store.onTelemetry(() => this.renderLive());
    store.onState(() => {
      this.editor.setEmergency(store.state?.emergencyTempC ?? 95);
      this.renderList();
    });
  }

  /** Called when the tab opens. */
  async show(): Promise<void> {
    await this.reload(this.selected);
  }

  private async reload(select: string | null): Promise<void> {
    try {
      [this.names, this.defaults] = await Promise.all([this.store.api.listCurves(), this.store.api.getDefaults()]);
    } catch (e) {
      toast(String(e));
      return;
    }
    const name = select && this.names.includes(select) ? select : this.names[0] ?? null;
    if (name) await this.open(name);
    this.renderList();
  }

  private async open(name: string): Promise<void> {
    try {
      this.curve = await this.store.api.getCurve(name);
    } catch (e) {
      toast(String(e));
      return;
    }
    this.selected = name;
    this.isNew = false;
    this.saved = JSON.stringify(this.curve);
    this.linked = same(this.curve.cpu, this.curve.gpu);
    this.active = 'cpu';
    this.renderList();
    this.renderEditor();
  }

  /** Loads `base` into the editor. With `newName`, it becomes a new curve. */
  private startFrom(base: Curve, newName?: string): void {
    this.curve = copy(base);
    const becomesNew = newName !== undefined || (!this.isNew && !this.selected);
    if (becomesNew) {
      this.isNew = true;
      this.selected = null;
      this.saved = null;
      this.nameInput.value = newName ?? '';
    }
    this.linked = same(this.curve.cpu, this.curve.gpu);
    this.renderList();
    this.renderEditor();
    if (becomesNew) this.nameInput.focus();
  }

  /** Where a curve runs now: a default's control, or the override. */
  private usersOf(name: string): string[] {
    const users: string[] = [];
    if (this.defaults?.ac.control === 'curve' && this.defaults.ac.curve === name) users.push('tomada');
    if (this.defaults?.battery.control === 'curve' && this.defaults.battery.curve === name) users.push('bateria');
    const s = this.store.state;
    if (s?.overrideActive && s.control === 'curve' && s.activeCurve === name) users.push('override');
    return users;
  }

  private renderList(): void {
    this.list.replaceChildren(
      ...this.names.map((name) => {
        const users = this.usersOf(name);
        const running = this.store.state?.control === 'curve' && this.store.state.activeCurve === name;
        const tag = running ? 'em uso' : users.length ? users.join(', ') : '';
        const li = html(`
          <li><button class="curve-item" aria-selected="${name === this.selected}">
            <span>${escape(name)}</span><span class="tag">${tag}</span>
          </button></li>`);
        $(li, 'button').addEventListener('click', () => void this.open(name));
        return li;
      }),
    );
    const del = $<HTMLButtonElement>(this.root, '[data-act="delete"]');
    const inUse = !this.selected || this.usersOf(this.selected).length > 0;
    del.disabled = inUse;
    del.title = inUse && this.selected ? 'A curva está em uso por um padrão ou pelo override.' : '';
    $<HTMLButtonElement>(this.root, '[data-act="duplicate"]').disabled = !this.curve;
  }

  private renderEditor(): void {
    const c = this.curve;
    $(this.root, '.editor').toggleAttribute('hidden', !c);
    if (!c) return;
    this.title.textContent = this.isNew ? 'Nova curva' : this.selected ?? '';
    this.nameInput.hidden = !this.isNew;
    $<HTMLInputElement>(this.root, '[data-linked]').checked = this.linked;
    const switcher = $(this.root, '.fan-switch');
    switcher.hidden = this.linked;
    switcher.querySelectorAll('button').forEach((b) => b.setAttribute('aria-pressed', String(b.dataset.fan === this.active)));
    this.root.querySelectorAll<HTMLInputElement>('[data-field]').forEach((input) => {
      input.value = String(c[input.dataset.field as 'hysteresisC' | 'rampUpPerS' | 'rampDownPerS']);
    });
    this.editor.set({ cpu: c.cpu, gpu: c.gpu }, this.linked, this.active);
    this.renderLive();
    this.renderDirty();
  }

  private renderLive(): void {
    const fans = this.store.telemetry?.fans ?? [];
    const live = Object.fromEntries(fans.map((f) => [f.id, { tempC: f.tempC, boost: f.boost }]));
    this.editor.setLive(live);
  }

  private renderDirty(): void {
    const dirty = this.isNew || JSON.stringify(this.curve) !== this.saved;
    $(this.root, '[data-dirty]').hidden = !dirty;
    const noName = this.isNew && !this.nameInput.value.trim();
    $<HTMLButtonElement>(this.root, '[data-act="save"]').disabled = !dirty || noName;
    $<HTMLButtonElement>(this.root, '[data-act="test"]').disabled = noName;
  }

  private onPoints(fan: FanId, points: Point[]): void {
    if (!this.curve) return;
    if (this.linked) {
      this.curve.cpu = points;
      this.curve.gpu = structuredClone(points);
    } else {
      this.curve[fan] = points;
    }
    this.editor.set({ cpu: this.curve.cpu, gpu: this.curve.gpu }, this.linked, this.active);
    this.renderDirty();
  }

  private currentName(): string {
    return this.isNew ? this.nameInput.value.trim() : this.selected ?? '';
  }

  private async save(): Promise<string | null> {
    const name = this.currentName();
    if (!this.curve || !name) return null;
    const ok = await this.store.act((api) => api.saveCurve(name, this.curve!), `Curva "${name}" salva.`);
    if (!ok) return null;
    this.isNew = false;
    await this.reload(name);
    return name;
  }

  private async onAction(e: Event): Promise<void> {
    const button = (e.target as Element).closest<HTMLButtonElement>('[data-act]');
    if (!button || button.disabled) return;
    switch (button.dataset.act) {
      case 'new':
        this.startFrom(PRESETS.Equilibrado, '');
        break;
      case 'duplicate':
        if (this.curve) this.startFrom(this.curve, `${this.currentName() || 'curva'}-copia`.slice(0, 32));
        break;
      case 'delete':
        // Two steps instead of a dialog: the first click asks, the second deletes.
        if (!this.selected) break;
        if (Date.now() - this.confirmDelete > 3000) {
          this.confirmDelete = Date.now();
          button.textContent = 'Confirmar';
          setTimeout(() => (button.textContent = 'Apagar'), 3000);
          break;
        }
        button.textContent = 'Apagar';
        this.confirmDelete = 0;
        {
          const name = this.selected;
          if (await this.store.act((api) => api.deleteCurve(name), `Curva "${name}" apagada.`)) {
            this.selected = null;
            await this.reload(null);
          }
        }
        break;
      case 'save':
        await this.save();
        break;
      case 'test': {
        const name = JSON.stringify(this.curve) === this.saved && !this.isNew ? this.selected : await this.save();
        if (name) await this.store.act((api) => api.setControl('curve', name), `Testando a curva "${name}".`);
        break;
      }
    }
  }
}
