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

import * as Animation from 'resource:///org/gnome/shell/ui/animation.js';
import * as Main from 'resource:///org/gnome/shell/ui/main.js';
import * as PanelMenu from 'resource:///org/gnome/shell/ui/panelMenu.js';
import * as PopupMenu from 'resource:///org/gnome/shell/ui/popupMenu.js';
import * as Slider from 'resource:///org/gnome/shell/ui/slider.js';

import {
    FAN_PROFILES, PROFILES, cycleProfile, daemonErrorText, fanProfileInfo, fanStatusText, formatAccel, formatMetric,
    normalizeOrder, profileInfo, pushSample, safetyEventText,
} from './format.js';
import {MenuGlassBlur} from './glassblur.js';
import {Sparkline} from './sparkline.js';

const APP_ID = 'org.blazorde.MsiCenter.desktop';
const APP_BUS_NAME = 'org.blazorde.MsiCenter';
const APP_OBJECT_PATH = '/org/blazorde/MsiCenter';
const CHARGE_MIN = 50;
const CHARGE_MAX = 100;
const CHARGE_DEBOUNCE_MS = 700; // keyboard / scroll changes have no "release": apply after a pause
const CONFIRM_MS = 1500; // keep the optimistic highlight until PropertiesChanged(Profile) lands
const SCROLL_COOLDOWN_MS = 350; // one profile step per wheel gesture
// Motion (Clutter has no CSS transforms): press feedback and error nudge, all ease-out, all < 200 ms.
const PRESS_SCALE = 0.96;
const PRESS_MS = 90;
const RELEASE_MS = 160;

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
        this._pending = null; // profile id being applied (optimistic)
        this._confirm = null; // applied, waiting for the Profile property to catch up
        this._confirmTimeout = 0;
        this._fanPending = null; // fan profile id being applied (optimistic)
        this._fanConfirm = null; // applied, waiting for the FanProfile property to catch up
        this._fanConfirmTimeout = 0;
        this._scrollAcc = 0;
        this._scrollLock = 0;

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
            if (key === 'show-clipboard-button')
                this._clipItem.visible = !this._settings.get_boolean('show-clipboard-button');
        }, this);
        this.connect('scroll-event', (_a, event) => this._onScroll(event));

        this._sync();
    }

    // ── Panel ──────────────────────────────────────────────────────────────

    _buildPanel() {
        this._panelBox = new St.BoxLayout({style_class: 'msi-panel-box', y_align: Clutter.ActorAlign.CENTER});
        // Profile chip: per-profile glyph + tint, short name. Offline: muted MSI logo, no name.
        this._chip = new St.BoxLayout({style_class: 'msi-chip-panel', y_align: Clutter.ActorAlign.CENTER});
        const iconFile = this._ext.dir.get_child('icons').get_child('msi-center-symbolic.svg');
        this._logo = new Gio.FileIcon({file: iconFile});
        this._icon = new St.Icon({gicon: this._logo, style_class: 'system-status-icon msi-panel-icon'});
        this._chipLabel = new St.Label({style_class: 'msi-chip-label', y_align: Clutter.ActorAlign.CENTER});
        this._chip.add_child(this._icon);
        this._chip.add_child(this._chipLabel);
        this._metricsBox = new St.BoxLayout({style_class: 'msi-metrics', y_align: Clutter.ActorAlign.CENTER});
        this._panelBox.add_child(this._chip);
        this._panelBox.add_child(this._metricsBox);
        this.add_child(this._panelBox);
        this._buildMetrics();
    }

    _buildMetrics() {
        this._metricsBox.destroy_all_children();
        this._metricItems = new Map();
        const iconOnly = this._settings.get_boolean('icon-only');
        this._chipLabel.visible = !iconOnly && this._daemon.available;
        if (iconOnly) {
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

        // 1. Profile switcher: the first and biggest thing in the menu. Three glass pills, one tap = SetProfile.
        const profItem = new PopupMenu.PopupBaseMenuItem({reactive: false, can_focus: false, style_class: 'msi-profiles-item'});
        const profBox = new St.BoxLayout({orientation: Clutter.Orientation.VERTICAL, x_expand: true, style_class: 'msi-profiles'});
        this._pillRow = new St.BoxLayout({style_class: 'msi-pill-row', x_expand: true});
        this._pillRow.layout_manager.homogeneous = true; // three equal pills whatever the label length
        this._pills = new Map();
        for (const p of PROFILES)
            this._pillRow.add_child(this._buildPill(p));
        // ← → move between pills; only at the row's ends do they reach PanelMenu (next top bar menu).
        const keys = new Clutter.KeyController();
        keys.connect('key-press', () => {
            const [, sym] = keys.get_key();
            const dir = sym === Clutter.KEY_Left ? St.DirectionType.LEFT
                : sym === Clutter.KEY_Right ? St.DirectionType.RIGHT : null;
            if (dir === null)
                return Clutter.EVENT_PROPAGATE;
            const moved = this._pillRow.navigate_focus(global.stage.key_focus, dir, false);
            return moved ? Clutter.EVENT_STOP : Clutter.EVENT_PROPAGATE;
        });
        this._pillRow.add_action(keys);
        profBox.add_child(this._pillRow);
        // Fan profile of the current profile: five compact chips, one tap = SetFanProfile. Hidden with an older
        // daemon that has no FanProfile property.
        this._fanRow = new St.BoxLayout({style_class: 'msi-fan-row', x_expand: true});
        this._fanRow.layout_manager.homogeneous = true;
        this._fanChips = new Map();
        for (const f of FAN_PROFILES)
            this._fanRow.add_child(this._buildFanChip(f));
        const fanKeys = new Clutter.KeyController();
        fanKeys.connect('key-press', () => {
            const [, sym] = fanKeys.get_key();
            const dir = sym === Clutter.KEY_Left ? St.DirectionType.LEFT
                : sym === Clutter.KEY_Right ? St.DirectionType.RIGHT : null;
            if (dir === null)
                return Clutter.EVENT_PROPAGATE;
            const moved = this._fanRow.navigate_focus(global.stage.key_focus, dir, false);
            return moved ? Clutter.EVENT_STOP : Clutter.EVENT_PROPAGATE;
        });
        this._fanRow.add_action(fanKeys);
        profBox.add_child(this._fanRow);
        // Live status under the pills: fan mode · power source · dGPU.
        this._subtitle = new St.Label({style_class: 'msi-status-line', x_align: Clutter.ActorAlign.CENTER});
        this._subtitle.clutter_text.ellipsize = Pango.EllipsizeMode.END;
        profBox.add_child(this._subtitle);
        profItem.add_child(profBox);
        this.menu.addMenuItem(profItem);

        // Banners (read-only, absent daemon, safety events, call errors).
        this._banners = new PopupMenu.PopupMenuSection();
        this.menu.addMenuItem(this._banners);

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

        // The clipboard has its own top bar button; this entry only appears when that button is hidden.
        this._clipItem = this.menu.addAction('Historique du presse-papiers', () => this._openClipboard());
        this._clipHint = new St.Label({style_class: 'msi-shortcut-hint', y_align: Clutter.ActorAlign.CENTER, x_expand: true, x_align: Clutter.ActorAlign.END});
        this._clipItem.add_child(this._clipHint);
        this._clipItem.visible = !this._settings.get_boolean('show-clipboard-button');
        this._syncShortcutHint();
        this._settings.connectObject('changed::clipboard-shortcut', () => this._syncShortcutHint(), this);

        this.menu.addAction('Ouvrir MSI Center', () => this._openApp());
        this.menu.addAction('Préférences de l’extension', () => this._ext.openPreferences());

        this.menu.connectObject('open-state-changed', (_m, open) => {
            if (open) {
                this._sync();
                this._spark.setSeries(this._cpuHistory, this._tempHistory);
                this._focusCurrentPill();
            }
        }, this);
    }

    _buildPill(p) {
        const pill = new St.Button({
            style_class: `msi-profile-pill msi-profile-${p.id}`,
            can_focus: true,
            x_expand: true,
            accessible_name: `Profil ${p.label} — ${p.caption}`,
        });
        const box = new St.BoxLayout({orientation: Clutter.Orientation.VERTICAL, style_class: 'msi-pill-box', x_expand: true});
        // Glyph bubble: the icon cross-fades with a spinner while SetProfile is in flight.
        const bubble = new St.Widget({style_class: 'msi-pill-bubble', layout_manager: new Clutter.BinLayout(), x_align: Clutter.ActorAlign.CENTER});
        const icon = new St.Icon({icon_name: p.icon, style_class: 'msi-pill-icon', x_align: Clutter.ActorAlign.CENTER, y_align: Clutter.ActorAlign.CENTER});
        const spinner = new Animation.Spinner(18, {hideOnStop: true});
        spinner.x_align = Clutter.ActorAlign.CENTER;
        spinner.y_align = Clutter.ActorAlign.CENTER;
        bubble.add_child(icon);
        bubble.add_child(spinner);
        box.add_child(bubble);
        box.add_child(new St.Label({text: p.label, style_class: 'msi-pill-label', x_align: Clutter.ActorAlign.CENTER}));
        const caption = new St.Label({text: p.caption, style_class: 'msi-pill-caption', x_align: Clutter.ActorAlign.CENTER});
        box.add_child(caption);
        pill.child = box;
        pill.set_pivot_point(0.5, 0.5);
        pill.connect('clicked', () => this._setProfile(p.id));
        // Tactile press: scale down quickly, release a bit slower.
        pill.connect('notify::pressed', () => {
            const down = pill.pressed;
            pill.ease({
                scale_x: down ? PRESS_SCALE : 1, scale_y: down ? PRESS_SCALE : 1,
                duration: down ? PRESS_MS : RELEASE_MS, mode: Clutter.AnimationMode.EASE_OUT_QUAD,
            });
        });
        this._pills.set(p.id, {pill, icon, spinner});
        return pill;
    }

    _buildFanChip(f) {
        const chip = new St.Button({
            style_class: `msi-fan-chip msi-fan-${f.id}`,
            can_focus: true,
            x_expand: true,
            accessible_name: `Ventilation ${f.label}`,
        });
        const box = new St.BoxLayout({orientation: Clutter.Orientation.VERTICAL, style_class: 'msi-fan-chip-box', x_expand: true});
        const bubble = new St.Widget({layout_manager: new Clutter.BinLayout(), x_align: Clutter.ActorAlign.CENTER});
        const icon = new St.Icon({icon_name: f.icon, style_class: 'msi-fan-chip-icon', x_align: Clutter.ActorAlign.CENTER, y_align: Clutter.ActorAlign.CENTER});
        const spinner = new Animation.Spinner(14, {hideOnStop: true});
        spinner.x_align = Clutter.ActorAlign.CENTER;
        spinner.y_align = Clutter.ActorAlign.CENTER;
        bubble.add_child(icon);
        bubble.add_child(spinner);
        box.add_child(bubble);
        box.add_child(new St.Label({text: f.short, style_class: 'msi-fan-chip-label', x_align: Clutter.ActorAlign.CENTER}));
        chip.child = box;
        chip.set_pivot_point(0.5, 0.5);
        chip.connect('clicked', () => this._setFanProfile(f.id));
        chip.connect('notify::pressed', () => {
            const down = chip.pressed;
            chip.ease({
                scale_x: down ? PRESS_SCALE : 1, scale_y: down ? PRESS_SCALE : 1,
                duration: down ? PRESS_MS : RELEASE_MS, mode: Clutter.AnimationMode.EASE_OUT_QUAD,
            });
        });
        this._fanChips.set(f.id, {chip, icon, spinner});
        return chip;
    }

    /** Fan profile to highlight: the one being applied, then the one just applied, then the daemon's. */
    _shownFanProfile() {
        return this._fanPending ?? this._fanConfirm ?? this._daemon.props.FanProfile ?? '';
    }

    _focusCurrentPill() {
        // Keyboard-opened menu: start on the active profile, not on the first pill.
        const focus = global.stage.key_focus;
        if (!focus || !this.menu.box.contains(focus))
            return;
        this._pills.get(this._shownProfile())?.pill.grab_key_focus();
    }

    _syncShortcutHint() {
        this._clipHint.text = formatAccel(this._settings.get_strv('clipboard-shortcut')[0] ?? '');
    }

    // ── State → view ───────────────────────────────────────────────────────

    _sync() {
        const live = this._daemon.available;
        const p = this._daemon.props;
        const readOnly = live && p.ReadOnly === true;
        const writable = live && !readOnly;

        if (this._confirm && p.Profile === this._confirm)
            this._clearConfirm();
        const shown = this._shownProfile();
        this._renderChip(live, shown);
        this._renderMetrics();

        if (this._fanConfirm && p.FanProfile === this._fanConfirm)
            this._clearFanConfirm();
        const parts = [];
        if (live) {
            const fans = fanStatusText(this._shownFanProfile(), p.FanMode);
            if (fans)
                parts.push(fans);
            parts.push(p.OnAcPower ? 'secteur' : 'batterie');
            if (p.GpuState)
                parts.push(`dGPU ${p.GpuState === 'active' ? 'actif' : p.GpuState === 'suspended' ? 'en veille' : 'retiré'}`);
        } else {
            parts.push('msi-daemon injoignable');
        }
        this._subtitle.text = parts.join(' · ');

        for (const [id, {pill, icon, spinner}] of this._pills) {
            pill.checked = live && shown === id;
            pill.reactive = writable;
            pill.can_focus = writable;
            const busy = this._pending === id;
            if (busy) {
                pill.add_style_pseudo_class('busy');
                spinner.play();
            } else {
                pill.remove_style_pseudo_class('busy');
                spinner.stop();
            }
            icon.ease({opacity: busy ? 0 : 255, duration: RELEASE_MS, mode: Clutter.AnimationMode.EASE_OUT_QUAD});
        }

        const fanShown = this._shownFanProfile();
        this._fanRow.visible = live && typeof p.FanProfile === 'string' && p.FanProfile !== '';
        for (const [id, {chip, icon, spinner}] of this._fanChips) {
            chip.checked = live && fanShown === id;
            chip.reactive = writable;
            chip.can_focus = writable;
            const isDefault = p.DefaultFanProfile === id;
            const info = fanProfileInfo(id);
            chip.accessible_name = `Ventilation ${info.label}${isDefault ? ' (par défaut)' : ''}`;
            if (isDefault)
                chip.add_style_pseudo_class('default');
            else
                chip.remove_style_pseudo_class('default');
            const busy = this._fanPending === id;
            if (busy) {
                chip.add_style_pseudo_class('busy');
                spinner.play();
            } else {
                chip.remove_style_pseudo_class('busy');
                spinner.stop();
            }
            icon.ease({opacity: busy ? 0 : 255, duration: RELEASE_MS, mode: Clutter.AnimationMode.EASE_OUT_QUAD});
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

    /** Profile to highlight: the one being applied, then the one just applied, then the daemon's. */
    _shownProfile() {
        return this._pending ?? this._confirm ?? this._daemon.props.Profile ?? '';
    }

    _renderChip(live, id) {
        const info = live ? profileInfo(id) : null;
        for (const p of PROFILES)
            this._chip.remove_style_class_name(`msi-profile-${p.id}`);
        if (info) {
            this._chip.add_style_class_name(`msi-profile-${info.id}`);
            this._icon.gicon = null;
            this._icon.icon_name = info.icon;
            this._icon.remove_style_class_name('msi-muted');
            this._chipLabel.text = info.label;
        } else {
            this._icon.gicon = this._logo;
            if (live)
                this._icon.remove_style_class_name('msi-muted');
            else
                this._icon.add_style_class_name('msi-muted');
            this._chipLabel.text = live ? id : '';
        }
        this._chipLabel.visible = live && !this._settings.get_boolean('icon-only') && this._chipLabel.text !== '';
        this.accessible_name = info ? `MSI Center, profil ${info.label}` : 'MSI Center';
    }

    _onScroll(event) {
        if (!this._settings.get_boolean('scroll-cycles-profile'))
            return Clutter.EVENT_PROPAGATE;
        let step = 0;
        switch (event.get_scroll_direction()) {
        case Clutter.ScrollDirection.UP:
            step = 1;
            break;
        case Clutter.ScrollDirection.DOWN:
            step = -1;
            break;
        case Clutter.ScrollDirection.SMOOTH: {
            const [, dy] = event.get_scroll_delta();
            this._scrollAcc -= dy;
            if (Math.abs(this._scrollAcc) >= 1) {
                step = Math.sign(this._scrollAcc);
                this._scrollAcc = 0;
            }
            break;
        }
        default:
            break;
        }
        if (step !== 0 && !this._scrollLock) {
            this._scrollLock = GLib.timeout_add(GLib.PRIORITY_DEFAULT, SCROLL_COOLDOWN_MS, () => {
                this._scrollLock = 0;
                return GLib.SOURCE_REMOVE;
            });
            this.cycleProfile(step, false);
        }
        return Clutter.EVENT_STOP;
    }

    /**
     * Step to the neighbour profile (scroll: clamped; shortcut: wraps). Shows the GNOME OSD when the menu is
     * closed so the change is visible without opening anything.
     */
    cycleProfile(delta, wrap) {
        const p = this._daemon.props;
        if (!this._daemon.available || p.ReadOnly === true || this._pending)
            return;
        const cur = this._shownProfile();
        const next = cycleProfile(cur, delta, wrap);
        if (next === cur)
            return;
        if (!this.menu.isOpen) {
            const info = profileInfo(next);
            const monitor = Main.layoutManager.findIndexForActor(this) ?? Main.layoutManager.primaryIndex;
            Main.osdWindowManager.showOne(monitor, Gio.ThemedIcon.new(info.icon), info.label, null);
        }
        this._setProfile(next);
    }

    _clearConfirm() {
        this._confirm = null;
        if (this._confirmTimeout)
            GLib.source_remove(this._confirmTimeout);
        this._confirmTimeout = 0;
    }

    _clearFanConfirm() {
        this._fanConfirm = null;
        if (this._fanConfirmTimeout)
            GLib.source_remove(this._fanConfirmTimeout);
        this._fanConfirmTimeout = 0;
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
        if (this._pending || !this._pills.has(id))
            return;
        const before = this._shownProfile();
        if (id === before)
            return;
        // Optimistic: highlight + spinner right away, revert to the daemon's value if the call fails.
        this._clearConfirm();
        this._pending = id;
        this._sync();
        let failed = false;
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
                failed = true;
                this._error = daemonErrorText(e.dbusName, e.message);
            }
        }
        if (!this._pills)
            return; // destroyed meanwhile
        this._pending = null;
        if (!failed && this._daemon.props.Profile !== id) {
            this._confirm = id;
            this._confirmTimeout = GLib.timeout_add(GLib.PRIORITY_DEFAULT, CONFIRM_MS, () => {
                this._confirmTimeout = 0;
                this._confirm = null;
                this._sync();
                return GLib.SOURCE_REMOVE;
            });
        }
        this._sync();
        if (failed)
            this._nudge(this._pills.get(id).pill);
    }

    async _setFanProfile(id) {
        if (this._fanPending || !this._fanChips?.has(id))
            return;
        if (id === this._shownFanProfile())
            return;
        // Optimistic like the pills: highlight + spinner now, back to the daemon's value on failure.
        this._clearFanConfirm();
        this._fanPending = id;
        this._sync();
        let failed = false;
        try {
            await this._daemon.setFanProfile(id);
            this._error = null;
        } catch (e) {
            failed = true;
            this._error = daemonErrorText(e.dbusName, e.message);
        }
        if (!this._fanChips)
            return; // destroyed meanwhile
        this._fanPending = null;
        if (!failed && this._daemon.props.FanProfile !== id) {
            this._fanConfirm = id;
            this._fanConfirmTimeout = GLib.timeout_add(GLib.PRIORITY_DEFAULT, CONFIRM_MS, () => {
                this._fanConfirmTimeout = 0;
                this._fanConfirm = null;
                this._sync();
                return GLib.SOURCE_REMOVE;
            });
        }
        this._sync();
        if (failed)
            this._nudge(this._fanChips.get(id).chip);
    }

    /** Small horizontal shake: "that did not work", without a modal. */
    _nudge(actor) {
        actor.remove_transition('translation-x');
        actor.translation_x = 6;
        actor.ease({translation_x: 0, duration: 260, mode: Clutter.AnimationMode.EASE_OUT_ELASTIC});
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
        for (const id of [this._chargeTimeout, this._confirmTimeout, this._fanConfirmTimeout, this._scrollLock]) {
            if (id)
                GLib.source_remove(id);
        }
        this._chargeTimeout = this._confirmTimeout = this._fanConfirmTimeout = this._scrollLock = 0;
        this._pills = null;
        this._fanChips = null;
        this._daemon.disconnectObject(this);
        this._settings.disconnectObject(this);
        this._glassBlur?.destroy();
        this._glassBlur = null;
        super.destroy();
    }
});
