// SPDX-License-Identifier: GPL-3.0-or-later
// Thin front-end only: no business logic here (see docs/adr/0004-gnome-extension-gjs.md). Implemented in phase 7.
import {Extension} from 'resource:///org/gnome/shell/extensions/extension.js';

export default class MsiCenterExtension extends Extension {
    enable() {}

    disable() {}
}
