// SPDX-License-Identifier: GPL-3.0-or-later
// Top bar shortcut to the clipboard history: icon only, optional count of session clips, click opens the
// Super+V popup dropped down under the button. No menu of its own (PanelMenu "dummy menu" mode).
import Clutter from 'gi://Clutter';
import GLib from 'gi://GLib';
import GObject from 'gi://GObject';
import St from 'gi://St';

import * as PanelMenu from 'resource:///org/gnome/shell/ui/panelMenu.js';

import {badgeText, formatAccel} from './format.js';

const BADGE_DEBOUNCE_MS = 400; // clipd emits Changed in bursts (capture + retention)

export const ClipboardButton = GObject.registerClass(
class ClipboardButton extends PanelMenu.Button {
    /**
     * @param {object} deps
     * @param {import('./clipd.js').ClipdClient} deps.clipd
     * @param {Gio.Settings} deps.settings
     * @param {import('./popup.js').ClipPopup} deps.popup
     */
    _init({clipd, settings, popup}) {
        super._init(0.5, 'Historique du presse-papiers', true);
        this._clipd = clipd;
        this._settings = settings;
        this._popup = popup;
        this._badgeTimeout = 0;
        this._seq = 0;
        this.add_style_class_name('msi-clip-button');

        const box = new St.BoxLayout({style_class: 'msi-clip-button-box', y_align: Clutter.ActorAlign.CENTER});
        box.add_child(new St.Icon({icon_name: 'edit-paste-symbolic', style_class: 'system-status-icon'}));
        this._badge = new St.Label({style_class: 'msi-clip-badge', y_align: Clutter.ActorAlign.CENTER, visible: false});
        box.add_child(this._badge);
        this.add_child(box);

        // dontCreateMenu disables PanelMenu's own click gesture: wire ours (pointer + touch) and Enter/Space.
        const click = new Clutter.ClickGesture();
        click.set_recognize_on_press(true);
        click.connect('recognize', () => this._popup.toggle({anchor: this}));
        this.add_action(click);
        const keys = new Clutter.KeyController();
        keys.connect('key-press', () => {
            const [, sym] = keys.get_key();
            if (![Clutter.KEY_Return, Clutter.KEY_KP_Enter, Clutter.KEY_ISO_Enter, Clutter.KEY_space].includes(sym))
                return Clutter.EVENT_PROPAGATE;
            this._popup.toggle({anchor: this});
            return Clutter.EVENT_STOP;
        });
        this.add_action(keys);

        this._popup.connectObject('open-state-changed', (_p, open) => {
            if (open && this._popup._anchor === this)
                this.add_style_pseudo_class('active');
            else
                this.remove_style_pseudo_class('active');
        }, this);
        this._clipd.connectObject(
            'changed', () => this._queueBadge(),
            'availability', () => this._queueBadge(),
            this);
        this._settings.connectObject(
            'changed::clipboard-badge', () => this._queueBadge(0),
            'changed::clipboard-shortcut', () => this._syncTooltip(),
            this);
        this._syncTooltip();
        this._queueBadge(0);
    }

    _syncTooltip() {
        const hint = formatAccel(this._settings.get_strv('clipboard-shortcut')[0] ?? '');
        this.accessible_name = `Historique du presse-papiers${hint ? ` (${hint})` : ''}`;
    }

    _queueBadge(ms = BADGE_DEBOUNCE_MS) {
        if (this._badgeTimeout)
            GLib.source_remove(this._badgeTimeout);
        this._badgeTimeout = GLib.timeout_add(GLib.PRIORITY_DEFAULT, ms, () => {
            this._badgeTimeout = 0;
            this._refreshBadge();
            return GLib.SOURCE_REMOVE;
        });
    }

    /** Session clips = all − pinned (non-pinned clips live in tmpfs and vanish at logout). */
    async _refreshBadge() {
        const seq = ++this._seq;
        if (!this._settings.get_boolean('clipboard-badge')) {
            this._badge.hide();
            return;
        }
        try {
            const [all, pinned] = await Promise.all([
                this._clipd.list('', [], 'all', 0, 1),
                this._clipd.list('', [], 'pinned', 0, 1),
            ]);
            if (seq !== this._seq || !this._badge)
                return;
            const text = badgeText(all.total - pinned.total);
            this._badge.text = text;
            this._badge.visible = text !== '';
        } catch {
            if (seq === this._seq && this._badge)
                this._badge.hide(); // clipd absent: the icon alone still opens the popup (which explains why)
        }
    }

    get badgeText() {
        return this._badge?.visible ? this._badge.text : '';
    }

    destroy() {
        if (this._badgeTimeout)
            GLib.source_remove(this._badgeTimeout);
        this._badgeTimeout = 0;
        this._popup.disconnectObject(this);
        this._clipd.disconnectObject(this);
        this._settings.disconnectObject(this);
        this._badge = null;
        super.destroy();
    }
});
