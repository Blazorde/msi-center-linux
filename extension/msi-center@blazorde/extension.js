// SPDX-License-Identifier: GPL-3.0-or-later
// Thin front-end only: no business logic here (see docs/adr/0004-gnome-extension-gjs.md).
// Wires D-Bus clients (msi-daemon, clipd) to the top bar indicators (profile + metrics, clipboard button),
// clipboard capture and the Super+V popup.
import Meta from 'gi://Meta';
import Shell from 'gi://Shell';

import {Extension} from 'resource:///org/gnome/shell/extensions/extension.js';
import * as Main from 'resource:///org/gnome/shell/ui/main.js';

import {ClipboardButton} from './lib/clipbutton.js';
import {ClipboardWatcher} from './lib/clipboard.js';
import {ClipdClient} from './lib/clipd.js';
import {DaemonClient} from './lib/daemon.js';
import {panelPlacement} from './lib/format.js';
import {MsiIndicator} from './lib/indicator.js';
import {ClipPopup} from './lib/popup.js';
import {ThemeWatcher} from './lib/theme.js';

const SHORTCUT_KEY = 'clipboard-shortcut';
const PROFILE_SHORTCUT_KEY = 'profile-cycle-shortcut';
const KEYBINDING_MODES = Shell.ActionMode.NORMAL | Shell.ActionMode.OVERVIEW | Shell.ActionMode.POPUP;

export default class MsiCenterExtension extends Extension {
    enable() {
        this._settings = this.getSettings();
        this._theme = new ThemeWatcher();
        this._daemon = new DaemonClient();
        this._clipd = new ClipdClient();
        this._watcher = new ClipboardWatcher(this._clipd, this._settings);
        this._popup = new ClipPopup({clipd: this._clipd, settings: this._settings, theme: this._theme});
        this._addIndicator();
        this._addClipButton();
        this._settings.connectObject(
            'changed::main-indicator-box', () => this._addIndicator(),
            'changed::main-indicator-index', () => this._addIndicator(),
            'changed::show-clipboard-button', () => this._addClipButton(),
            'changed::clipboard-button-box', () => this._addClipButton(),
            'changed::clipboard-button-index', () => this._addClipButton(),
            this);

        Main.wm.addKeybinding(SHORTCUT_KEY, this._settings,
            Meta.KeyBindingFlags.IGNORE_AUTOREPEAT, KEYBINDING_MODES,
            () => this._popup.toggle());
        Main.wm.addKeybinding(PROFILE_SHORTCUT_KEY, this._settings,
            Meta.KeyBindingFlags.IGNORE_AUTOREPEAT, KEYBINDING_MODES,
            () => this._indicator?.cycleProfile(1, true));
    }

    /** (Re)create the profile + metrics indicator at its configured place (default: left, after Activities). */
    _addIndicator() {
        this._indicator?.destroy();
        this._indicator = new MsiIndicator({
            extension: this,
            daemon: this._daemon,
            settings: this._settings,
            theme: this._theme,
            openClipboard: () => this._popup.open(),
        });
        const {box, index} = panelPlacement(this._settings.get_string('main-indicator-box'),
            this._settings.get_int('main-indicator-index'), 'left');
        Main.panel.addToStatusArea(this.uuid, this._indicator, index, box);
    }

    /** (Re)create the clipboard button (default: right, first of the system area), or drop it if hidden. */
    _addClipButton() {
        this._clipButton?.destroy();
        this._clipButton = null;
        if (!this._settings.get_boolean('show-clipboard-button'))
            return;
        this._clipButton = new ClipboardButton({clipd: this._clipd, settings: this._settings, popup: this._popup});
        const {box, index} = panelPlacement(this._settings.get_string('clipboard-button-box'),
            this._settings.get_int('clipboard-button-index'), 'right');
        Main.panel.addToStatusArea(`${this.uuid}-clipboard`, this._clipButton, index, box);
    }

    disable() {
        // Also called on screen lock (session-modes: user only), so everything is torn down.
        Main.wm.removeKeybinding(SHORTCUT_KEY);
        Main.wm.removeKeybinding(PROFILE_SHORTCUT_KEY);
        this._settings.disconnectObject(this);
        this._clipButton?.destroy();
        this._clipButton = null;
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
