// SPDX-License-Identifier: GPL-3.0-or-later
// Frosted-glass backing for a popup menu.
//
// Why a separate actor: PopupMenu's BoxPointer renders its subtree offscreen (offscreen_redirect ALWAYS),
// so a Shell.BlurEffect in BACKGROUND mode inside the menu sees nothing. We put a blur actor in uiGroup just
// below the menu and track the glass box. Clutter 51 has no rounded clipping, so the blur rectangle is inset
// by ~0.3 × corner radius: a rectangle inset by r·(1 − 1/√2) always fits inside the rounded box, so no
// square blurred corners poke out; the thin unblurred rim reads as the glass edge.
import Clutter from 'gi://Clutter';
import Meta from 'gi://Meta';
import Shell from 'gi://Shell';
import St from 'gi://St';

import * as Main from 'resource:///org/gnome/shell/ui/main.js';

const CORNER_RADIUS = 26; // keep in sync with `.msi-glass { border-radius }` in stylesheet.css
const INSET = Math.ceil(CORNER_RADIUS * (1 - Math.SQRT1_2));
const FADE_MS = 140;

export class MenuGlassBlur {
    /**
     * @param {PopupMenu.PopupMenu} menu
     * @param {Gio.Settings} settings  uses the `blur` key
     */
    constructor(menu, settings) {
        this._menu = menu;
        this._settings = settings;
        this._later = 0;
        this._actor = new St.Widget({style_class: 'msi-menu-blur', visible: false, reactive: false});
        this._actor.add_effect_with_name('msi-blur', new Shell.BlurEffect({
            mode: Shell.BlurMode.BACKGROUND, radius: 28, brightness: 1.0,
        }));
        Main.uiGroup.insert_child_below(this._actor, menu.actor);

        menu.connectObject('open-state-changed', (_m, open) => (open ? this._show() : this._hide()), this);
        menu.box.connectObject('notify::allocation', () => this._queueSync(), this);
        // BoxPointer slides in with translation_x/y: follow it so the blur lands under the final position.
        menu.actor.connectObject(
            'notify::translation-x', () => this._queueSync(),
            'notify::translation-y', () => this._queueSync(),
            'destroy', () => this.destroy(),
            this);
        settings.connectObject('changed::blur', () => {
            if (!settings.get_boolean('blur'))
                this._hide();
            else if (menu.isOpen)
                this._show();
        }, this);
    }

    _show() {
        if (!this._settings.get_boolean('blur'))
            return;
        this._queueSync();
        this._actor.opacity = 0;
        this._actor.show();
        this._actor.ease({opacity: 255, duration: FADE_MS, mode: Clutter.AnimationMode.EASE_OUT_QUAD});
    }

    _hide() {
        this._actor.ease({
            opacity: 0, duration: FADE_MS, mode: Clutter.AnimationMode.EASE_IN_QUAD,
            onStopped: () => this._actor?.hide(),
        });
    }

    _queueSync() {
        if (!this._actor || this._later || !this._menu.isOpen)
            return;
        const laters = global.compositor.get_laters();
        this._later = laters.add(Meta.LaterType.BEFORE_REDRAW, () => {
            this._later = 0;
            this._sync();
            return false;
        });
    }

    _sync() {
        if (!this._actor || !this._menu.isOpen)
            return;
        const box = this._menu.box;
        const [x, y] = box.get_transformed_position();
        const [w, h] = box.get_transformed_size();
        if (w <= 2 * INSET || h <= 2 * INSET)
            return;
        this._actor.set_position(Math.round(x + INSET), Math.round(y + INSET));
        this._actor.set_size(Math.round(w - 2 * INSET), Math.round(h - 2 * INSET));
        Main.uiGroup.set_child_below_sibling(this._actor, this._menu.actor);
    }

    destroy() {
        if (!this._actor)
            return;
        if (this._later)
            global.compositor.get_laters().remove(this._later);
        this._later = 0;
        this._menu.disconnectObject(this);
        this._menu.box.disconnectObject(this);
        this._menu.actor.disconnectObject(this);
        this._settings.disconnectObject(this);
        this._actor.destroy();
        this._actor = null;
    }
}
