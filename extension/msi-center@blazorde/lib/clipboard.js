// SPDX-License-Identifier: GPL-3.0-or-later
// Clipboard capture: Wayland selection (Meta.Selection `owner-changed`) → St.Clipboard read → clipd `Add`.
// Owner's choice: everything is captured, including password-manager copies (no "sensitive" filtering).
import GLib from 'gi://GLib';
import Meta from 'gi://Meta';
import St from 'gi://St';

import {pickCaptureMime} from './format.js';

const DEBOUNCE_MS = 80; // apps sometimes re-own the selection several times per copy

export class ClipboardWatcher {
    /**
     * @param {import('./clipd.js').ClipdClient} clipd
     * @param {Gio.Settings} settings
     */
    constructor(clipd, settings) {
        this._clipd = clipd;
        this._settings = settings;
        this._clipboard = St.Clipboard.get_default();
        this._selection = global.display.get_selection();
        this._timeout = 0;
        this._warned = false;
        this._ownerId = this._selection.connect('owner-changed', (_sel, type) => {
            if (type === Meta.SelectionType.SELECTION_CLIPBOARD)
                this._schedule();
        });
    }

    _schedule() {
        if (!this._settings.get_boolean('clipboard-capture'))
            return;
        if (this._timeout)
            GLib.source_remove(this._timeout);
        this._timeout = GLib.timeout_add(GLib.PRIORITY_DEFAULT, DEBOUNCE_MS, () => {
            this._timeout = 0;
            this._capture();
            return GLib.SOURCE_REMOVE;
        });
    }

    _capture() {
        const mimes = this._clipboard.get_mimetypes(St.ClipboardType.CLIPBOARD) ?? [];
        const pick = pickCaptureMime(mimes);
        if (!pick)
            return;
        if (pick.kind === 'text') {
            this._clipboard.get_text(St.ClipboardType.CLIPBOARD, (_cb, text) => {
                if (!text)
                    return;
                const bytes = new GLib.Bytes(new TextEncoder().encode(text));
                this._send('text/plain', bytes);
            });
        } else {
            this._clipboard.get_content(St.ClipboardType.CLIPBOARD, pick.mime, (_cb, bytes) => {
                if (bytes && bytes.get_size() > 0)
                    this._send(pick.mime, bytes);
            });
        }
    }

    _send(mime, bytes) {
        if (bytes.get_size() > this._clipd.maxClipBytes) {
            console.debug(`msi-center: clip ignored, ${bytes.get_size()} bytes > MaxClipBytes`);
            return;
        }
        this._clipd.add(mime, bytes).then(() => {
            this._warned = false;
        }).catch(e => {
            // Rejected (empty, too big, invalid UTF-8) is expected; log the rest once until it recovers.
            if (e.dbusName?.endsWith('.Rejected'))
                return;
            if (!this._warned)
                console.warn(`msi-center: clipd Add failed: ${e.message}`);
            this._warned = true;
        });
    }

    destroy() {
        if (this._timeout)
            GLib.source_remove(this._timeout);
        this._timeout = 0;
        this._selection.disconnect(this._ownerId);
    }
}
