// SPDX-License-Identifier: GPL-3.0-or-later
// Top bar indicator + menu. Pure view over DaemonClient: every action is a single D-Bus call, every state
// shown comes from properties / signals. No thresholds, no decisions.
import Clutter from 'gi://Clutter';
import Gio from 'gi://Gio';
import GLib from 'gi://GLib';
import GObject from 'gi://GObject';
import Pango from 'gi://Pango';
import Shell from 'gi://Shell';
import St from 'gi://St';

import * as Main from 'resource:///org/gnome/shell/ui/main.js';
import * as PanelMenu from 'resource:///org/gnome/shell/ui/panelMenu.js';
import * as PopupMenu from 'resource:///org/gnome/shell/ui/popupMenu.js';
import * as Slider from 'resource:///org/gnome/shell/ui/slider.js';

import {
    PROFILES, daemonErrorText, formatMetric, normalizeOrder, profileLabel, pushSample, safetyEventText,
} from './format.js';
import {MenuGlassBlur} from './glassblur.js';
import {Sparkline} from './sparkline.js';

const APP_ID = 'org.blazorde.MsiCenter.desktop';
const APP_BUS_NAME = 'org.blazorde.MsiCenter';
const APP_OBJECT_PATH = '/org/blazorde/MsiCenter';
const CHARGE_MIN = 50;
const CHARGE_MAX = 100;
const CHARGE_DEBOUNCE_MS = 700; // keyboard / scroll changes have no "release": apply after a pause

export const MsiIndicator = GObject.registerClass(
class MsiIndicator extends PanelMenu.Button {
    /**
     * @param {object} deps
     * @param {import('resource:///org/gnome/shell/extensions/extension.js').Extension} deps.extension
     * @param {import('./daemon.js').DaemonClient} deps.daemon
     * @param {Gio.Settings} deps.settings
     * @param {import('./theme.js').ThemeWatcher} deps.theme
     * @param {Function} deps.openClipboard
     */
    _init({extension, daemon, settings, theme, openClipboard}) {
        super._init(0.5, 'MSI Center');
        this._ext = extension;
        this._daemon = daemon;
        this._settings = settings;
        this._theme = theme;
        this._openClipboard = openClipboard;
        this._telemetry = {};
        this._cpuHistory = [];
        this._tempHistory = [];
        this._events = new Map(); // group → {level, text}
        this._error = null; // last failed call, French text
        this._chargeTimeout = 0;
        this._chargeDragging = false;

        this.add_style_class_name('msi-panel-button');
        this._buildPanel();
        this._buildMenu();

        this._daemon.connectObject(
            'availability', () => this._sync(),
            'properties', () => this._sync(),
            'telemetry', (_d, t) => this._onTelemetry(t),
            'safety-event', (_d, kind, value, detail) => this._onSafetyEvent(kind, value, detail),
            this);

        this._settings.connectObject('changed', (_s, key) => {
            if (key.startsWith('show-') || key === 'metric-order' || key === 'icon-only')
                this._buildMetrics();
            if (key === 'show-sparkline')
                this._sparkItem.visible = this._settings.get_boolean('show-sparkline');
        }, this);

        this._sync();
    }

    // ── Panel ──────────────────────────────────────────────────────────────

    _buildPanel() {
        this._panelBox = new St.BoxLayout({style_class: 'msi-panel-box', y_align: Clutter.ActorAlign.CENTER});
        const iconFile = this._ext.dir.get_child('icons').get_child('msi-center-symbolic.svg');
        this._icon = new St.Icon({
            gicon: new Gio.FileIcon({file: iconFile}),
            style_class: 'system-status-icon msi-panel-icon',
        });
        this._metricsBox = new St.BoxLayout({style_class: 'msi-metrics', y_align: Clutter.ActorAlign.CENTER});
        this._panelBox.add_child(this._icon);
        this._panelBox.add_child(this._metricsBox);
        this.add_child(this._panelBox);
        this._buildMetrics();
    }

    _buildMetrics() {
        this._metricsBox.destroy_all_children();
        this._metricItems = new Map();
        if (this._settings.get_boolean('icon-only')) {
            this._metricsBox.hide();
            return;
        }
        this._metricsBox.show();
        const order = normalizeOrder(this._settings.get_strv('metric-order'));
        for (const id of order) {
            if (!this._settings.get_boolean(`show-${id}`))
                continue;
            const box = new St.BoxLayout({style_class: `msi-metric msi-metric-${id}`, y_align: Clutter.ActorAlign.CENTER});
            const prefix = new St.Label({style_class: 'msi-metric-prefix', y_align: Clutter.ActorAlign.CENTER});
            const value = new St.Label({style_class: 'msi-metric-value', y_align: Clutter.ActorAlign.CENTER});
            box.add_child(prefix);
            box.add_child(value);
            box.hide();
            this._metricsBox.add_child(box);
            this._metricItems.set(id, {box, prefix, value});
        }
        this._renderMetrics();
    }

    _renderMetrics() {
        const live = this._daemon.available;
        for (const [id, item] of this._metricItems ?? []) {
            const m = live ? formatMetric(id, this._telemetry, this._daemon.props.GpuState) : null;
            if (!m) {
                item.box.hide();
                continue;
            }
            item.prefix.text = m.prefix;
            item.prefix.visible = m.prefix.length > 0;
            item.value.text = m.value;
            item.box.show();
        }
    }

    _onTelemetry(t) {
        this._telemetry = t;
        this._renderMetrics();
        pushSample(this._cpuHistory, t.cpu_pct);
        pushSample(this._tempHistory, t.cpu_temp);
        if (this.menu.isOpen && this._sparkItem.visible)
            this._spark.setSeries(this._cpuHistory, this._tempHistory);
        if (typeof t.cooler_boost === 'boolean' && this.menu.isOpen)
            this._boostItem.setToggleState(t.cooler_boost);
    }

    // ── Menu ───────────────────────────────────────────────────────────────

    _buildMenu() {
        const box = this.menu.box;
        box.add_style_class_name('msi-glass');
        box.add_style_class_name('msi-menu');
        this._theme.track(box);
        this._glassBlur = new MenuGlassBlur(this.menu, this._settings);

        // Header: sunset gradient strip with title and live status.
        const header = new PopupMenu.PopupBaseMenuItem({reactive: false, can_focus: false, style_class: 'msi-header-item'});
        const headerBox = new St.BoxLayout({orientation: Clutter.Orientation.VERTICAL, style_class: 'msi-header', x_expand: true});
        const titleRow = new St.BoxLayout({style_class: 'msi-header-row'});
        titleRow.add_child(new St.Label({text: 'MSI Center', style_class: 'msi-title', x_expand: true}));
        this._profilePill = new St.Label({style_class: 'msi-pill', y_align: Clutter.ActorAlign.CENTER});
        titleRow.add_child(this._profilePill);
        this._subtitle = new St.Label({style_class: 'msi-subtitle'});
        headerBox.add_child(titleRow);
        headerBox.add_child(this._subtitle);
        header.add_child(headerBox);
        this.menu.addMenuItem(header);

        // Banners (read-only, absent daemon, safety events, call errors).
        this._banners = new PopupMenu.PopupMenuSection();
        this.menu.addMenuItem(this._banners);

        // Segmented profile switch.
        const segItem = new PopupMenu.PopupBaseMenuItem({reactive: false, can_focus: false, style_class: 'msi-segment-item'});
        this._segmented = new St.BoxLayout({style_class: 'msi-segmented', x_expand: true});
        this._segments = new Map();
        for (const p of PROFILES) {
            const b = new St.Button({
                label: p.label,
                style_class: `msi-segment msi-segment-${p.id}`,
                can_focus: true,
                x_expand: true,
            });
            b.connect('clicked', () => this._setProfile(p.id));
            this._segmented.add_child(b);
            this._segments.set(p.id, b);
        }
        segItem.add_child(this._segmented);
        this.menu.addMenuItem(segItem);

        // Live sparkline (CPU % + CPU °C).
        this._sparkItem = new PopupMenu.PopupBaseMenuItem({reactive: false, can_focus: false, style_class: 'msi-spark-item'});
        this._spark = new Sparkline();
        this._sparkItem.add_child(this._spark);
        this._sparkItem.visible = this._settings.get_boolean('show-sparkline');
        this.menu.addMenuItem(this._sparkItem);

        this.menu.addMenuItem(new PopupMenu.PopupSeparatorMenuItem());

        this._boostItem = new PopupMenu.PopupSwitchMenuItem('Cooler Boost', false);
        this._boostItem.connect('toggled', (_i, on) => this._run(() => this._daemon.setCoolerBoost(on)));
        this.menu.addMenuItem(this._boostItem);

        this._autoItem = new PopupMenu.PopupSwitchMenuItem('Profil auto secteur / batterie', false);
        this._autoItem.connect('toggled', (_i, on) => this._run(() => this._daemon.setAutoProfileOnPower(on)));
        this.menu.addMenuItem(this._autoItem);

        // Charge limit: applied on release (drag-end) or after a pause for keyboard/scroll.
        this._chargeItem = new PopupMenu.PopupBaseMenuItem({activate: false, style_class: 'msi-charge-item'});
        const chargeBox = new St.BoxLayout({orientation: Clutter.Orientation.VERTICAL, x_expand: true});
        const chargeRow = new St.BoxLayout();
        chargeRow.add_child(new St.Label({text: 'Limite de charge', x_expand: true, style_class: 'msi-row-label'}));
        this._chargeValue = new St.Label({style_class: 'msi-row-value'});
        chargeRow.add_child(this._chargeValue);
        this._chargeSlider = new Slider.Slider(0.6);
        this._chargeSlider.add_style_class_name('msi-slider');
        this._chargeSlider.accessible_name = 'Limite de charge';
        this._chargeSlider.connect('notify::value', () => this._onChargeMoved());
        this._chargeSlider.connect('drag-begin', () => {
            this._chargeDragging = true;
        });
        this._chargeSlider.connect('drag-end', () => {
            this._chargeDragging = false;
            this._applyCharge();
        });
        chargeBox.add_child(chargeRow);
        chargeBox.add_child(this._chargeSlider);
        this._chargeItem.add_child(chargeBox);
        this.menu.addMenuItem(this._chargeItem);

        this.menu.addMenuItem(new PopupMenu.PopupSeparatorMenuItem());

        const clipItem = this.menu.addAction('Historique du presse-papiers', () => this._openClipboard());
        this._clipHint = new St.Label({style_class: 'msi-shortcut-hint', y_align: Clutter.ActorAlign.CENTER, x_expand: true, x_align: Clutter.ActorAlign.END});
        clipItem.add_child(this._clipHint);
        this._syncShortcutHint();
        this._settings.connectObject('changed::clipboard-shortcut', () => this._syncShortcutHint(), this);

        this.menu.addAction('Ouvrir MSI Center', () => this._openApp());
        this.menu.addAction('Préférences de l’extension', () => this._ext.openPreferences());

        this.menu.connectObject('open-state-changed', (_m, open) => {
            if (open) {
                this._sync();
                this._spark.setSeries(this._cpuHistory, this._tempHistory);
            }
        }, this);
    }

    _syncShortcutHint() {
        const accel = this._settings.get_strv('clipboard-shortcut')[0] ?? '';
        this._clipHint.text = accel.replace(/<Super>/i, 'Super+').replace(/<Control>|<Primary>/i, 'Ctrl+')
            .replace(/<Shift>/i, 'Maj+').replace(/<Alt>/i, 'Alt+').replace(/\+([a-z])$/, (_m, c) => `+${c.toUpperCase()}`);
    }

    // ── State → view ───────────────────────────────────────────────────────

    _sync() {
        const live = this._daemon.available;
        const p = this._daemon.props;
        const readOnly = live && p.ReadOnly === true;
        const writable = live && !readOnly;

        if (live)
            this._icon.remove_style_class_name('msi-muted');
        else
            this._icon.add_style_class_name('msi-muted');
        this._renderMetrics();

        this._profilePill.text = live ? profileLabel(p.Profile) : 'hors ligne';
        const parts = [];
        if (live) {
            if (p.FanMode)
                parts.push(p.FanMode === 'advanced' ? 'courbes actives' : `ventilos : ${p.FanMode}`);
            parts.push(p.OnAcPower ? 'secteur' : 'batterie');
            if (p.GpuState)
                parts.push(`dGPU ${p.GpuState === 'active' ? 'actif' : p.GpuState === 'suspended' ? 'en veille' : 'retiré'}`);
        } else {
            parts.push('msi-daemon injoignable');
        }
        this._subtitle.text = parts.join(' · ');

        for (const [id, b] of this._segments) {
            b.checked = live && p.Profile === id;
            b.reactive = writable;
            b.can_focus = writable;
        }

        this._boostItem.setToggleState(live && p.CoolerBoost === true);
        // Cannot release a boost forced by the thermal supervisor (S4): the daemon would refuse anyway.
        this._boostItem.setSensitive(writable && p.BoostForced !== true);
        this._boostItem.label.text = p.BoostForced ? 'Cooler Boost (forcé)' : 'Cooler Boost';

        this._autoItem.setToggleState(live && p.AutoProfileOnPower === true);
        this._autoItem.setSensitive(writable);

        const limit = p.ChargeLimit ?? 0;
        this._chargeItem.visible = live && limit > 0;
        if (limit > 0 && !this._chargeDragging && !this._chargeTimeout) {
            this._chargeSlider.value = (limit - CHARGE_MIN) / (CHARGE_MAX - CHARGE_MIN);
            this._chargeValue.text = `${limit} %`;
        }
        this._chargeSlider.reactive = writable;

        this._renderBanners(live, readOnly, p.ReadOnlyReason);
    }

    _renderBanners(live, readOnly, reason) {
        this._banners.removeAll();
        const add = (level, text, onClick = null) => {
            const item = new PopupMenu.PopupBaseMenuItem({
                reactive: onClick !== null, can_focus: onClick !== null,
                style_class: `msi-banner msi-banner-${level}`,
            });
            const label = new St.Label({text, x_expand: true, style_class: 'msi-banner-text'});
            label.clutter_text.line_wrap = true;
            label.clutter_text.ellipsize = Pango.EllipsizeMode.NONE;
            item.add_child(label);
            if (onClick)
                item.connect('activate', onClick);
            this._banners.addMenuItem(item);
        };
        if (!live)
            add('muted', 'msi-daemon est injoignable. Reconnexion automatique dès qu’il revient.');
        else if (readOnly)
            add('warning', `Lecture seule (courbe usine)${reason ? ` : ${reason}` : ''}`);
        for (const [group, ev] of this._events) {
            add(ev.level, ev.text, () => {
                this._events.delete(group);
                this._renderBanners(this._daemon.available, this._daemon.props.ReadOnly === true,
                    this._daemon.props.ReadOnlyReason);
            });
        }
        if (this._error) {
            add('error', this._error, () => {
                this._error = null;
                this._sync();
            });
        }
    }

    _onSafetyEvent(kind, value, detail) {
        const ev = safetyEventText(kind, value, detail);
        const group = kind.startsWith('boost_') ? 'thermal' : kind === 'gpu_busy' ? 'gpu' : 'ec';
        this._events.set(group, ev);
        if (ev.level === 'error')
            Main.notify('MSI Center', ev.text);
        this._sync();
    }

    // ── Actions ────────────────────────────────────────────────────────────

    async _run(fn) {
        try {
            await fn();
            this._error = null;
        } catch (e) {
            this._error = daemonErrorText(e.dbusName, e.message);
        }
        this._sync();
    }

    async _setProfile(id) {
        const b = this._segments.get(id);
        b.add_style_class_name('msi-pending');
        try {
            await this._daemon.setProfile(id);
            this._error = null;
            this._events.delete('gpu');
        } catch (e) {
            if (e.dbusName?.endsWith('.GpuBusy')) {
                let who = '';
                try {
                    const holders = await this._daemon.gpuHolders();
                    who = holders.map(([pid, name]) => `${name} (${pid})`).join(', ');
                } catch {
                    // Holders are informative only.
                }
                this._events.set('gpu', {
                    level: 'warning',
                    text: `Profil appliqué, dGPU laissé allumé${who ? ` : utilisé par ${who}` : ''}.`,
                });
                this._error = null;
            } else {
                this._error = daemonErrorText(e.dbusName, e.message);
            }
        } finally {
            b.remove_style_class_name('msi-pending');
        }
        this._sync();
    }

    _chargeFromSlider() {
        return Math.round(CHARGE_MIN + this._chargeSlider.value * (CHARGE_MAX - CHARGE_MIN));
    }

    _onChargeMoved() {
        this._chargeValue.text = `${this._chargeFromSlider()} %`;
        if (this._chargeDragging)
            return;
        if (this._chargeTimeout)
            GLib.source_remove(this._chargeTimeout);
        this._chargeTimeout = GLib.timeout_add(GLib.PRIORITY_DEFAULT, CHARGE_DEBOUNCE_MS, () => {
            this._chargeTimeout = 0;
            this._applyCharge();
            return GLib.SOURCE_REMOVE;
        });
    }

    _applyCharge() {
        if (this._chargeTimeout) {
            GLib.source_remove(this._chargeTimeout);
            this._chargeTimeout = 0;
        }
        const limit = this._chargeFromSlider();
        if (limit === this._daemon.props.ChargeLimit)
            return;
        this._run(() => this._daemon.setChargeLimit(limit));
    }

    /** Launch the GTK app: desktop app info first, else `org.freedesktop.Application.Activate`. */
    _openApp() {
        const app = Shell.AppSystem.get_default().lookup_app(APP_ID);
        if (app) {
            app.activate();
            return;
        }
        // Session-bus activation of the user app (never the root daemon).
        Gio.DBus.session.call(APP_BUS_NAME, APP_OBJECT_PATH, 'org.freedesktop.Application', 'Activate',
            new GLib.Variant('(a{sv})', [{}]), null, Gio.DBusCallFlags.NONE, -1, null)
            .catch(e => Main.notify('MSI Center', `Impossible de lancer l’application : ${e.message}`));
    }

    destroy() {
        if (this._chargeTimeout)
            GLib.source_remove(this._chargeTimeout);
        this._chargeTimeout = 0;
        this._daemon.disconnectObject(this);
        this._settings.disconnectObject(this);
        this._glassBlur?.destroy();
        this._glassBlur = null;
        super.destroy();
    }
});
