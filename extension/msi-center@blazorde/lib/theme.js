// SPDX-License-Identifier: GPL-3.0-or-later
// Light/dark follow-up. GNOME Shell loads a single extension stylesheet, and St CSS has no variables, so
// stylesheet.css holds both variants under `.msi-dark` / `.msi-light`; this tags our root actors.
import St from 'gi://St';

import * as Main from 'resource:///org/gnome/shell/ui/main.js';

export class ThemeWatcher {
    constructor() {
        this._actors = new Set();
        this._settings = St.Settings.get();
        this._id = this._settings.connect('notify::color-scheme', () => this._applyAll());
    }

    get variant() {
        return Main.getStyleVariant() === 'light' ? 'light' : 'dark';
    }

    /** Track an actor; untracked automatically on destroy. */
    track(actor) {
        this._actors.add(actor);
        actor.connect('destroy', () => this._actors.delete(actor));
        this._apply(actor);
    }

    _apply(actor) {
        const light = this.variant === 'light';
        actor.remove_style_class_name(light ? 'msi-dark' : 'msi-light');
        actor.add_style_class_name(light ? 'msi-light' : 'msi-dark');
    }

    _applyAll() {
        for (const a of this._actors)
            this._apply(a);
    }

    destroy() {
        this._settings.disconnect(this._id);
        this._actors.clear();
    }
}
