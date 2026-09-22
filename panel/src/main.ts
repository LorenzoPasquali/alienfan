// Entry point: tabs, banner, status bar and the three views.

import '@fontsource/inter/400.css';
import '@fontsource/inter/500.css';
import '@fontsource/inter/600.css';
import '@fontsource/jetbrains-mono/400.css';
import '@fontsource/jetbrains-mono/500.css';
import './styles.css';

import { connect } from './api';
import { HEALTH, POWER } from './labels';
import { Store } from './store';
import { $, rovingKeys } from './ui';
import { CurvesView } from './views/curves';
import { DefaultsView } from './views/defaults';
import { FansView } from './views/fans';

/** While the daemon is away, look for it this often (SPEC 13.1). */
const RECONNECT_MS = 2000;

// `?theme=dark|light` forces a theme, for previews and screenshots.
const theme = new URLSearchParams(location.search).get('theme');
if (theme === 'dark' || theme === 'light') document.documentElement.dataset.theme = theme;

const store = new Store(await connect());
new FansView($(document, '#view-fans'), store);
const curves = new CurvesView($(document, '#view-curves'), store);
const defaults = new DefaultsView($(document, '#view-defaults'), store);

// ---------- Tabs ----------

const tablist = $(document, '[role="tablist"]');
const onShow: Record<string, () => void> = {
  'view-curves': () => void curves.show(),
  'view-defaults': () => void defaults.show(),
};

function selectTab(tab: HTMLElement): void {
  for (const t of tablist.querySelectorAll<HTMLElement>('[role="tab"]')) {
    const selected = t === tab;
    t.setAttribute('aria-selected', String(selected));
    t.tabIndex = selected ? 0 : -1;
    $(document, `#${t.getAttribute('aria-controls')}`).hidden = !selected;
  }
  onShow[tab.getAttribute('aria-controls') ?? '']?.();
}

tablist.querySelectorAll<HTMLElement>('[role="tab"]').forEach((t) => t.addEventListener('click', () => selectTab(t)));
rovingKeys(tablist, '[role="tab"]', selectTab);

// ---------- Banner, status bar ----------

const banner = $(document, '#banner');
const bannerText = $(document, '#banner-text');
const bannerAction = $<HTMLButtonElement>(document, '#banner-action');

bannerAction.addEventListener('click', () => void store.act((api) => api.startDaemon(), 'Serviço iniciado.'));
$(document, '#restore-default').addEventListener('click', () => void store.act((api) => api.restoreDefault()));
$(document, '#save-default').addEventListener('click', () => {
  const source = store.state?.powerSource;
  if (source) void store.act((api) => api.saveAsDefault(source), 'Padrão salvo.');
});

function render(): void {
  const s = store.state;
  const daemon = $(document, '#daemon-status');
  $(daemon, '.dot').className = `dot ${s ? 'is-ok' : 'is-bad'}`;
  daemon.lastElementChild!.textContent = s ? 'Serviço conectado' : 'Serviço parado';

  const problem = !s || s.health === 'no-permission' || s.health === 'no-driver';
  banner.hidden = !problem;
  banner.classList.toggle('is-error', !s || s.health === 'no-driver');
  bannerAction.hidden = !!s;
  if (!s) {
    bannerText.textContent = 'O serviço alienfand não está rodando. Nada muda até ele voltar.';
  } else if (problem) {
    bannerText.innerHTML = '';
    bannerText.append(`${s.healthMessage} `);
    const code = document.createElement('code');
    code.textContent = 'alienfan doctor';
    bannerText.append('Diagnóstico: ', code);
  }

  const power = $(document, '#status-power');
  power.textContent = s ? POWER[s.powerSource] : '';
  $(document, '#status-power').hidden = !s;
  $(document, '#status-health').hidden = !s;
  const health = $(document, '#status-health');
  const level = !s ? '' : s.health === 'ok' ? 'is-ok' : s.health === 'degraded' ? 'is-warn' : 'is-bad';
  $(health, '.dot').className = `dot ${level}`;
  health.lastElementChild!.textContent = s ? HEALTH[s.health] : '';
  health.title = s?.healthMessage ?? '';

  const override = $(document, '#status-override');
  override.hidden = !s?.overrideActive;
  if (s) $(override, '#save-default').textContent = `Salvar como padrão (${POWER[s.powerSource].toLowerCase()})`;
  document.body.dataset.connected = String(!!s);
}

store.onState(render);
await store.refresh();
setInterval(() => {
  if (!store.connected) void store.refresh();
}, RECONNECT_MS);
