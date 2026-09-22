// "Ventoinhas": profile strip and one card per fan (SPEC 13.2).

import type { FanId, FanTelemetry } from '../api';
import { fromPct, toPct } from '../curve-math';
import { FanView } from '../fan';
import { CONTROL, FAN_NAME, PROFILE_ORDER, band, icon, profileInfo } from '../labels';
import type { Store } from '../store';
import { $, debounce, html, rovingKeys } from '../ui';

const SLIDER_DELAY_MS = 150;
const STEP_PCT = 10;

export class FansView {
  private readonly segmented: HTMLElement;
  private readonly indicator: HTMLElement;
  private readonly hint: HTMLElement;
  private readonly cards: Record<FanId, FanCard>;
  private profileKeys = '';

  constructor(root: HTMLElement, private readonly store: Store) {
    root.append(
      html(`
        <div class="profiles">
          <div class="segmented" role="radiogroup" aria-label="Perfil térmico">
            <span class="seg-indicator" aria-hidden="true"></span>
          </div>
          <p class="profile-hint"></p>
        </div>`),
      html('<div class="cards"></div>'),
    );
    this.segmented = $(root, '.segmented');
    this.indicator = $(root, '.seg-indicator');
    this.hint = $(root, '.profile-hint');
    const cards = $(root, '.cards');
    this.cards = { cpu: new FanCard('cpu', store), gpu: new FanCard('gpu', store) };
    cards.append(this.cards.cpu.el, this.cards.gpu.el);

    rovingKeys(this.segmented, '.seg', (el) => this.pick(el.dataset.profile!));
    new ResizeObserver(() => this.moveIndicator()).observe(this.segmented);
    store.onState(() => this.syncState());
    store.onTelemetry(() => this.syncTelemetry());
  }

  private pick(profile: string): void {
    if (profile === this.store.state?.profile) return;
    void this.store.act((api) => api.setProfile(profile));
  }

  private syncState(): void {
    const s = this.store.state;
    const available = PROFILE_ORDER.filter((p) => s?.availableProfiles.includes(p));
    if (available.join() !== this.profileKeys) {
      this.profileKeys = available.join();
      this.segmented.querySelectorAll('.seg').forEach((b) => b.remove());
      for (const p of available) {
        const info = profileInfo(p);
        const button = html<HTMLButtonElement>(`
          <button class="seg" role="radio" data-profile="${p}" title="${info.hint}">
            ${icon(info.icon as Parameters<typeof icon>[0])}<span>${info.name}</span>
            ${info.badge ? `<span class="badge">${info.badge}</span>` : ''}
          </button>`);
        button.addEventListener('click', () => this.pick(p));
        this.segmented.append(button);
      }
    }
    // With a boost that needs `custom`, the profile is not the user's pick.
    const locked = !!s && s.boostRequiresCustom && s.control !== 'firmware';
    for (const b of this.segmented.querySelectorAll<HTMLButtonElement>('.seg')) {
      const checked = b.dataset.profile === s?.profile;
      b.setAttribute('aria-checked', String(checked));
      b.tabIndex = checked ? 0 : -1;
      b.disabled = !s || locked;
    }
    $(this.segmented.parentElement!, '.segmented').parentElement!.hidden = !s;
    const info = s ? profileInfo(s.profile) : null;
    const tip = s?.profile === 'quiet'
      ? 'O boost só acelera: para menos ruído, fique no Silencioso sem boost.'
      : 'Para deixar mais silencioso, use Silencioso. O boost só acelera.';
    this.hint.textContent = locked ? 'Boost ativo usa o perfil Personalizado.' : `${info?.hint ?? ''} ${tip}`;
    this.moveIndicator();
    this.cards.cpu.syncState();
    this.cards.gpu.syncState();
  }

  private moveIndicator(): void {
    const checked = this.segmented.querySelector<HTMLElement>('.seg[aria-checked="true"]');
    this.indicator.hidden = !checked;
    if (!checked) return;
    this.indicator.style.width = `${checked.offsetWidth}px`;
    this.indicator.style.transform = `translateX(${checked.offsetLeft}px)`;
    // Slide between profiles, but not into the first position.
    if (!this.indicator.classList.contains('is-placed')) {
      requestAnimationFrame(() => this.indicator.classList.add('is-placed'));
    }
  }

  private syncTelemetry(): void {
    for (const f of this.store.telemetry?.fans ?? []) this.cards[f.id]?.syncTelemetry(f);
  }
}

class FanCard {
  readonly el: HTMLElement;
  private readonly fan: FanView;
  private readonly slider: HTMLInputElement;
  private readonly send: ReturnType<typeof debounce<[number]>>;
  private telemetry: FanTelemetry | null = null;
  private dragging = false;

  constructor(private readonly id: FanId, private readonly store: Store) {
    const name = FAN_NAME[id];
    this.el = html(`
      <article class="card fan-card" data-fan="${id}" aria-label="Ventoinha da ${name}">
        <div class="fan-stage">
          <div class="ring"></div>
          <div class="ring-pointer" aria-hidden="true"></div>
        </div>
        <div class="fan-head"><h2>${name}</h2><span class="chip"></span></div>
        <div class="readout">
          <div class="rpm" aria-live="off"><span class="rpm-value">0</span><small>rpm</small></div>
          <div class="temp"></div>
          <p class="fan-note" hidden>Ventoinha da GPU parada: normal com a GPU ociosa.</p>
        </div>
        <div class="fan-controls">
          <label class="boost-label" for="boost-${id}">Boost <output class="num"></output></label>
          <input id="boost-${id}" type="range" min="0" max="100" step="1" value="0"
            aria-label="Boost da ventoinha da ${name}, em porcentagem" />
          <div class="boost-buttons">
            <button class="btn step-down" aria-label="Diminuir boost em 10%">−10%</button>
            <button class="btn step-up" aria-label="Aumentar boost em 10%">+10%</button>
            <button class="btn take-over" hidden>Assumir manualmente</button>
            <button class="btn btn-quiet firmware">Automático (firmware)</button>
          </div>
        </div>
      </article>`);
    this.fan = new FanView(id);
    $(this.el, '.fan-stage').append(this.fan.svg);
    this.slider = $(this.el, 'input');
    this.send = debounce(SLIDER_DELAY_MS, (pct: number) => {
      void this.store.act((api) => api.setFixedBoost(this.id, fromPct(pct)));
    });

    this.slider.addEventListener('pointerdown', () => (this.dragging = true));
    this.slider.addEventListener('pointerup', () => (this.dragging = false));
    this.slider.addEventListener('pointercancel', () => (this.dragging = false));
    this.slider.addEventListener('input', () => {
      this.showSlider(Number(this.slider.value));
      this.send(Number(this.slider.value));
    });
    $(this.el, '.step-down').addEventListener('click', () => this.step(-STEP_PCT));
    $(this.el, '.step-up').addEventListener('click', () => this.step(STEP_PCT));
    $(this.el, '.firmware').addEventListener('click', () => {
      void this.store.act((api) => api.setControl('firmware'));
    });
    $(this.el, '.take-over').addEventListener('click', () => {
      void this.store.act((api) => api.setControl('fixed'));
    });
  }

  private step(delta: number): void {
    const pct = Math.min(100, Math.max(0, toPct(this.telemetry?.boost ?? 0) + delta));
    this.showSlider(pct);
    void this.store.act((api) => api.setFixedBoost(this.id, fromPct(pct)));
  }

  private showSlider(pct: number): void {
    this.slider.value = String(pct);
    this.slider.style.setProperty('--fill', `${pct}%`);
    $(this.el, 'output').textContent = `${pct}%`;
  }

  syncState(): void {
    const s = this.store.state;
    const curve = s?.control === 'curve';
    this.slider.disabled = !s || curve;
    this.el.querySelectorAll<HTMLButtonElement>('.btn').forEach((b) => (b.disabled = !s));
    $(this.el, '.take-over').hidden = !curve;
    $(this.el, '.firmware').hidden = s?.control === 'firmware';
    ($(this.el, '.step-down') as HTMLButtonElement).disabled = !s || curve;
    ($(this.el, '.step-up') as HTMLButtonElement).disabled = !s || curve;
    this.el.toggleAttribute('data-emergency', s?.health === 'emergency');
    this.syncChip();
  }

  private syncChip(): void {
    const s = this.store.state;
    const chip = $(this.el, '.chip');
    if (!s) {
      chip.textContent = 'Sem serviço';
    } else if (s.health === 'emergency') {
      chip.textContent = 'Emergência';
    } else if (s.control === 'fixed') {
      chip.textContent = `Manual ${toPct(this.telemetry?.boost ?? 0)}%`;
    } else if (s.control === 'curve') {
      chip.textContent = `Curva ${s.activeCurve}`;
    } else {
      chip.textContent = CONTROL.firmware;
    }
  }

  syncTelemetry(f: FanTelemetry): void {
    this.telemetry = f;
    const ratio = f.rpmMax > 0 ? Math.min(1, f.rpm / f.rpmMax) : 0;
    this.el.dataset.band = band(f.tempC);
    ($(this.el, '.fan-stage') as HTMLElement).style.setProperty('--ratio', ratio.toFixed(3));
    this.fan.setRpm(f.rpm, f.rpmMax);
    $(this.el, '.rpm-value').textContent = String(f.rpm);
    $(this.el, '.temp').textContent = f.tempC === null ? 'sem leitura' : `${Math.round(f.tempC)} °C`;
    $(this.el, '.fan-note').hidden = !(f.id === 'gpu' && f.rpm === 0 && f.boost === 0);
    // The curve shows what it wants; manual shows what was written. Never
    // fight the user's hand on the slider.
    const shown = this.store.state?.control === 'curve' ? f.targetBoost : f.boost;
    if (!this.send.pending() && !this.dragging) this.showSlider(toPct(shown));
    this.syncChip();
  }
}
