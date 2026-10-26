// SPDX-License-Identifier: GPL-3.0-or-later
// Thin front-end only: no business logic here (see docs/adr/0004-gnome-extension-gjs.md).
// Wires D-Bus clients (msi-daemon, clipd) to the top bar indicator, clipboard capture and Super+V popup.
import Meta from 'gi://Meta';
import Shell from 'gi://Shell';

import {Extension} from 'resource:///org/gnome/shell/extensions/extension.js';
import * as Main from 'resource:///org/gnome/shell/ui/main.js';

import {ClipboardWatcher} from './lib/clipboard.js';
import {ClipdClient} from './lib/clipd.js';
import {DaemonClient} from './lib/daemon.js';
import {MsiIndicator} from './lib/indicator.js';
import {ClipPopup} from './lib/popup.js';
import {ThemeWatcher} from './lib/theme.js';

const SHORTCUT_KEY = 'clipboard-shortcut';

export default class MsiCenterExtension extends Extension {
    enable() {
        this._settings = this.getSettings();
        this._theme = new ThemeWatcher();
        this._daemon = new DaemonClient();
        this._clipd = new ClipdClient();
        this._watcher = new ClipboardWatcher(this._clipd, this._settings);
        this._popup = new ClipPopup({clipd: this._clipd, settings: this._settings, theme: this._theme});
        this._indicator = new MsiIndicator({
            extension: this,
            daemon: this._daemon,
            settings: this._settings,
            theme: this._theme,
            openClipboard: () => this._popup.open(),
        });
        Main.panel.addToStatusArea(this.uuid, this._indicator);

        Main.wm.addKeybinding(SHORTCUT_KEY, this._settings,
            Meta.KeyBindingFlags.IGNORE_AUTOREPEAT,
            Shell.ActionMode.NORMAL | Shell.ActionMode.OVERVIEW | Shell.ActionMode.POPUP,
            () => this._popup.toggle());
    }

    disable() {
        // Also called on screen lock (session-modes: user only), so everything is torn down.
        Main.wm.removeKeybinding(SHORTCUT_KEY);
        this._indicator?.destroy();
        this._indicator = null;
        this._popup?.destroy();
        this._popup = null;
        this._watcher?.destroy();
        this._watcher = null;
        this._clipd?.destroy();
        this._clipd = null;
        this._daemon?.destroy();
        this._daemon = null;
        this._theme?.destroy();
        this._theme = null;
        this._settings = null;
    }
}
