// alienfan: Quick Settings toggle for the Alienware thermal profile and
// fans (SPEC 12). Uses only GNOME Shell 46 APIs.

import Clutter from 'gi://Clutter';
import GLib from 'gi://GLib';
import GObject from 'gi://GObject';
import Gio from 'gi://Gio';
import St from 'gi://St';

import {Extension} from 'resource:///org/gnome/shell/extensions/extension.js';
import * as Main from 'resource:///org/gnome/shell/ui/main.js';
import * as PopupMenu from 'resource:///org/gnome/shell/ui/popupMenu.js';
import {QuickMenuToggle, SystemIndicator} from 'resource:///org/gnome/shell/ui/quickSettings.js';
import {Slider} from 'resource:///org/gnome/shell/ui/slider.js';

import {createProxy, errorMessage, unpackDict} from './dbus.js';

const TITLE = 'Ventoinhas';
const PANEL_DESKTOP_ID = 'io.github.lorenzopasquali.AlienFan.desktop';
/** Wait this long after the slider stops before calling the daemon. */
const SLIDER_DELAY_MS = 150;

const PROFILE_NAMES = {
    'cool': 'Frio',
    'quiet': 'Silencioso',
    'balanced': 'Equilibrado',
    'balanced-performance': 'Equilibrado+',
    'performance': 'Desempenho (G-Mode)',
    'custom': 'Personalizado',
};

const POWER_NAMES = {ac: 'Na tomada', battery: 'Na bateria'};

const HEALTH_NAMES = {
    'ok': 'tudo certo',
    'degraded': 'degradado',
    'emergency': 'emergência',
    'no-permission': 'sem permissão',
    'no-driver': 'sem driver',
};

const pct = boost => Math.round(boost * 100 / 255);

/** A labelled slider for one fan's boost, 0–100%. */
const BoostItem = GObject.registerClass(
class BoostItem extends PopupMenu.PopupBaseMenuItem {
    _init(fan, onChange) {
        super._init({activate: false, style_class: 'alienfan-boost-item'});

        this._fan = fan;
        this._rpm = 0;
        this._timeoutId = 0;

        this.add_child(new St.Label({
            text: fan.toUpperCase(),
            y_align: Clutter.ActorAlign.CENTER,
            style_class: 'alienfan-boost-name',
        }));
        this.slider = new Slider(0);
        this.slider.accessible_name = `Boost da ventoinha ${fan.toUpperCase()}`;
        this.add_child(this.slider);
        this._value = new St.Label({
            y_align: Clutter.ActorAlign.CENTER,
            style_class: 'alienfan-boost-value',
        });
        this.add_child(this._value);

        this._valueId = this.slider.connect('notify::value', () => {
            this._syncLabel();
            this._schedule(onChange);
        });
        this._syncLabel();
        this.connect('destroy', () => this._cancel());
    }

    /** Shows what the daemon reports, unless the user is moving the slider. */
    setBoost(boost, rpm) {
        this._rpm = rpm;
        if (!this._timeoutId) {
            this.slider.block_signal_handler(this._valueId);
            this.slider.value = boost / 255;
            this.slider.unblock_signal_handler(this._valueId);
        }
        this._syncLabel();
    }

    _syncLabel() {
        this._value.text = `${Math.round(this.slider.value * 100)}% · ${this._rpm} rpm`;
    }

    _schedule(onChange) {
        this._cancel();
        this._timeoutId = GLib.timeout_add(GLib.PRIORITY_DEFAULT, SLIDER_DELAY_MS, () => {
            this._timeoutId = 0;
            onChange(this._fan, Math.round(this.slider.value * 255));
            return GLib.SOURCE_REMOVE;
        });
    }

    _cancel() {
        if (this._timeoutId) {
            GLib.source_remove(this._timeoutId);
            this._timeoutId = 0;
        }
    }
});

const FanToggle = GObject.registerClass(
class FanToggle extends QuickMenuToggle {
    /**
     * @param {Gio.Icon} icon - the propeller icon
     * @param {Function} onAttention - called with true when the panel icon
     *   should show (override active or health not ok)
     */
    _init(icon, onAttention) {
        super._init({title: TITLE, gicon: icon, toggleMode: false});

        this._icon = icon;
        this._onAttention = onAttention;
        this._proxy = null;
        this._proxyIds = [];
        this._telemetryId = 0;
        this._profileItems = new Map();
        this._boost = {cpu: 0, gpu: 0};
        this._cancellable = new Gio.Cancellable();

        this._buildMenu();
        this.connect('clicked', () => this._onClicked());
        this.menu.connect('open-state-changed', (_menu, open) => this._onMenuOpen(open));
        this.connect('destroy', () => this._onDestroy());

        createProxy((proxy, error) => {
            if (error) {
                if (!error.matches(Gio.IOErrorEnum, Gio.IOErrorEnum.CANCELLED))
                    console.error(`alienfan: ${error.message}`);
                return;
            }
            this._proxy = proxy;
            this._proxyIds = [
                proxy.connect('g-properties-changed', () => this._sync()),
                proxy.connect('notify::g-name-owner', () => this._sync()),
            ];
            this._sync();
        }, this._cancellable);
        this._sync();
    }

    _buildMenu() {
        this.menu.setHeader(this._icon, TITLE, '');

        this._healthItem = new PopupMenu.PopupMenuItem('', {reactive: false});
        this._healthItem.add_style_class_name('alienfan-health');
        this.menu.addMenuItem(this._healthItem);
        this._startItem = this.menu.addAction('Iniciar serviço', () => this._startService());

        this.menu.addMenuItem(new PopupMenu.PopupSeparatorMenuItem('Perfil'));
        this._profileSection = new PopupMenu.PopupMenuSection();
        this.menu.addMenuItem(this._profileSection);
        this._profileNote = new PopupMenu.PopupMenuItem('Boost ativo usa Personalizado',
            {reactive: false});
        this._profileNote.add_style_class_name('alienfan-note');
        this.menu.addMenuItem(this._profileNote);

        this.menu.addMenuItem(new PopupMenu.PopupSeparatorMenuItem('Controle'));
        this._firmwareItem = this.menu.addAction('Automático (firmware)',
            () => this._call('SetControl', 'firmware', ''));
        this._manualItem = this.menu.addAction('Manual',
            () => this._call('SetControl', 'fixed', ''));
        this._curveMenu = new PopupMenu.PopupSubMenuMenuItem('Curva');
        this.menu.addMenuItem(this._curveMenu);

        this._boostSection = new PopupMenu.PopupMenuSection();
        this._boostSection.addMenuItem(new PopupMenu.PopupSeparatorMenuItem('Boost'));
        const onBoost = (fan, boost) => this._call('SetFixedBoost', fan, boost);
        this._boostItems = {cpu: new BoostItem('cpu', onBoost), gpu: new BoostItem('gpu', onBoost)};
        this._boostSection.addMenuItem(this._boostItems.cpu);
        this._boostSection.addMenuItem(this._boostItems.gpu);
        this.menu.addMenuItem(this._boostSection);

        this._telemetryItem = new PopupMenu.PopupMenuItem('', {reactive: false});
        this._telemetryItem.add_style_class_name('alienfan-telemetry');
        this.menu.addMenuItem(this._telemetryItem);

        this.menu.addMenuItem(new PopupMenu.PopupSeparatorMenuItem());
        this._saveItem = this.menu.addAction('Salvar como padrão',
            () => this._call('SaveAsDefault', this._proxy.PowerSource));
        this._restoreItem = this.menu.addAction('Voltar ao padrão',
            () => this._call('RestoreDefault'));
        this.menu.addAction('Abrir painel…', () => this._openPanel());

        this._daemonItems = [
            this._firmwareItem, this._manualItem, this._curveMenu,
            this._saveItem, this._restoreItem,
        ];
    }

    _running() {
        return !!this._proxy?.g_name_owner;
    }

    _sync() {
        const running = this._running();
        this._startItem.visible = !running;
        this._daemonItems.forEach(item => item.setSensitive(running));
        this._boostSection.actor.visible = running && this._proxy.Control === 'fixed';

        if (!running) {
            this.set({subtitle: 'Serviço parado', checked: false});
            this.menu.setHeader(this._icon, TITLE, 'Serviço parado');
            this._healthItem.visible = false;
            this._profileNote.visible = false;
            this._profileSection.removeAll();
            this._profileItems.clear();
            this._telemetryItem.label.text = '';
            this._onAttention(false);
            return;
        }

        const p = this._proxy;
        const healthy = p.Health === 'ok';
        this.set({subtitle: this._subtitle(), checked: p.OverrideActive});
        this.menu.setHeader(this._icon, TITLE,
            `${POWER_NAMES[p.PowerSource] ?? p.PowerSource} · ${HEALTH_NAMES[p.Health] ?? p.Health}`);
        this._onAttention(p.OverrideActive || !healthy);

        this._healthItem.visible = !healthy;
        this._healthItem.label.text = p.HealthMessage;
        if (p.Health === 'emergency')
            this._healthItem.add_style_class_name('alienfan-emergency');
        else
            this._healthItem.remove_style_class_name('alienfan-emergency');

        this._syncProfiles();
        const control = p.Control;
        this._firmwareItem.setOrnament(control === 'firmware'
            ? PopupMenu.Ornament.CHECK : PopupMenu.Ornament.NONE);
        this._manualItem.setOrnament(control === 'fixed'
            ? PopupMenu.Ornament.CHECK : PopupMenu.Ornament.NONE);
        this._curveMenu.label.text = control === 'curve' ? `Curva: ${p.ActiveCurve}` : 'Curva';
        this._saveItem.label.text =
            `Salvar como padrão (${(POWER_NAMES[p.PowerSource] ?? p.PowerSource).toLowerCase()})`;
        this._restoreItem.setSensitive(p.OverrideActive);

        // The subtitle shows the boost, so keep it fresh with the menu closed.
        if (control === 'fixed' && !this._telemetryId)
            this._refreshTelemetry();
    }

    _subtitle() {
        const p = this._proxy;
        const profile = PROFILE_NAMES[p.Profile] ?? p.Profile;
        if (p.Control === 'curve')
            return `${profile} · Curva ${p.ActiveCurve}`;
        if (p.Control === 'fixed') {
            const [cpu, gpu] = [pct(this._boost.cpu), pct(this._boost.gpu)];
            return `${profile} · Boost ${cpu === gpu ? cpu : `${cpu}/${gpu}`}%`;
        }
        return profile;
    }

    _syncProfiles() {
        const p = this._proxy;
        const available = p.AvailableProfiles;
        if (available.join(' ') !== [...this._profileItems.keys()].join(' ')) {
            this._profileSection.removeAll();
            this._profileItems.clear();
            for (const profile of available) {
                const item = new PopupMenu.PopupMenuItem(PROFILE_NAMES[profile] ?? profile);
                item.connect('activate', () => this._call('SetProfile', profile));
                this._profileSection.addMenuItem(item);
                this._profileItems.set(profile, item);
            }
        }
        // With a boost that needs `custom`, the profile is not the user's pick.
        const locked = p.BoostRequiresCustom && p.Control !== 'firmware';
        this._profileNote.visible = locked;
        for (const [profile, item] of this._profileItems) {
            item.setSensitive(!locked);
            item.setOrnament(profile === p.Profile
                ? PopupMenu.Ornament.CHECK : PopupMenu.Ornament.NONE);
        }
    }

    _fillCurves() {
        this._proxy.ListCurvesAsync().then(([names]) => {
            this._curveMenu.menu.removeAll();
            for (const name of names) {
                const item = this._curveMenu.menu.addAction(name,
                    () => this._call('SetControl', 'curve', name));
                item.setOrnament(this._proxy.Control === 'curve' && this._proxy.ActiveCurve === name
                    ? PopupMenu.Ornament.CHECK : PopupMenu.Ornament.NONE);
            }
        }).catch(e => console.error(`alienfan: ${errorMessage(e)}`));
    }

    _onMenuOpen(open) {
        if (open && this._running() && !this._telemetryId) {
            // Telemetry only while someone looks at it (SPEC 12.3).
            this._telemetryId = this._proxy.connectSignal('Telemetry',
                (_proxy, _sender, [fans]) => this._onTelemetry(fans));
            this._refreshTelemetry();
            this._fillCurves();
        } else if (!open) {
            this._disconnectTelemetry();
        }
    }

    _refreshTelemetry() {
        this._proxy.GetTelemetryAsync()
            .then(([fans]) => this._onTelemetry(fans))
            .catch(e => console.error(`alienfan: ${errorMessage(e)}`));
    }

    _onTelemetry(rawFans) {
        const fans = rawFans.map(unpackDict);
        const parts = [];
        for (const fan of fans) {
            const temp = fan.temp_c === undefined ? '?' : `${Math.round(fan.temp_c)} °C`;
            parts.push(`${fan.id.toUpperCase()} ${temp} · ${fan.rpm} rpm`);
            this._boost[fan.id] = fan.boost;
            this._boostItems[fan.id]?.setBoost(fan.boost, fan.rpm);
        }
        this._telemetryItem.label.text = parts.join('   ');
        if (this._running())
            this.subtitle = this._subtitle();
    }

    _disconnectTelemetry() {
        if (this._telemetryId) {
            this._proxy?.disconnectSignal(this._telemetryId);
            this._telemetryId = 0;
        }
    }

    _onClicked() {
        if (this._running() && this._proxy.OverrideActive)
            this._call('RestoreDefault');
        else
            this.menu.open();
    }

    _call(method, ...args) {
        if (!this._running())
            return;
        this._proxy[`${method}Async`](...args)
            .catch(e => Main.notifyError(TITLE, errorMessage(e)));
    }

    _startService() {
        try {
            const proc = Gio.Subprocess.new(
                ['systemctl', '--user', 'start', 'alienfand.service'],
                Gio.SubprocessFlags.STDERR_PIPE);
            proc.communicate_utf8_async(null, this._cancellable, (p, res) => {
                try {
                    const [, , stderr] = p.communicate_utf8_finish(res);
                    if (!p.get_successful())
                        Main.notifyError(TITLE, `Não foi possível iniciar o serviço: ${stderr.trim()}`);
                } catch (e) {
                    if (!e.matches?.(Gio.IOErrorEnum, Gio.IOErrorEnum.CANCELLED))
                        console.error(`alienfan: ${e.message}`);
                }
            });
        } catch (e) {
            Main.notifyError(TITLE, e.message);
        }
    }

    _openPanel() {
        try {
            const app = Gio.DesktopAppInfo.new(PANEL_DESKTOP_ID);
            if (app)
                app.launch([], null);
            else
                Gio.Subprocess.new(['alienfan-panel'], Gio.SubprocessFlags.NONE);
        } catch (e) {
            Main.notifyError(TITLE, `Não foi possível abrir o painel: ${e.message}`);
        }
        Main.panel.closeQuickSettings();
    }

    _onDestroy() {
        this._cancellable.cancel();
        this._disconnectTelemetry();
        this._proxyIds.forEach(id => this._proxy.disconnect(id));
        this._proxyIds = [];
        this._proxy = null;
        // Quick Settings keeps the menu actor in its overlay otherwise.
        this.menu.destroy();
    }
});

const FanIndicator = GObject.registerClass(
class FanIndicator extends SystemIndicator {
    _init(icon) {
        super._init();

        this._panelIcon = this._addIndicator();
        this._panelIcon.gicon = icon;
        this._panelIcon.visible = false;
        this.quickSettingsItems.push(new FanToggle(icon, attention => {
            this._panelIcon.visible = attention;
        }));
    }
});

export default class AlienFanExtension extends Extension {
    enable() {
        const icon = Gio.icon_new_for_string(`${this.path}/icons/alienfan-symbolic.svg`);
        this._indicator = new FanIndicator(icon);
        Main.panel.statusArea.quickSettings.addExternalIndicator(this._indicator);
    }

    disable() {
        this._indicator.quickSettingsItems.forEach(item => item.destroy());
        this._indicator.destroy();
        this._indicator = null;
    }
}
