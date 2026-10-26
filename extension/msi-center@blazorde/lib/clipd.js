// SPDX-License-Identifier: GPL-3.0-or-later
// Client for `org.blazorde.Clipd1` (session bus, clipd). Contract: docs/architecture/dbus-api.md.
// Every call may D-Bus-activate clipd, so a missing service is retried naturally on the next call.
import Gio from 'gi://Gio';
import GLib from 'gi://GLib';

import {EventEmitter} from 'resource:///org/gnome/shell/misc/signals.js';

const BUS_NAME = 'org.blazorde.Clipd1';
const OBJECT_PATH = '/org/blazorde/Clipd';
const IFACE = 'org.blazorde.Clipd1';
const DEFAULT_MAX_CLIP_BYTES = 10 * 1024 * 1024;

/**
 * Events: `availability` (bool), `changed` (clipd `Changed` signal).
 */
export class ClipdClient extends EventEmitter {
    constructor() {
        super();
        this._bus = Gio.DBus.session;
        this.available = false;
        this.maxClipBytes = DEFAULT_MAX_CLIP_BYTES;
        this._cancellable = new Gio.Cancellable();
        this._sub = this._bus.signal_subscribe(BUS_NAME, IFACE, 'Changed', OBJECT_PATH, null,
            Gio.DBusSignalFlags.NONE, () => this.emit('changed'));
        this._watchId = Gio.bus_watch_name(Gio.BusType.SESSION, BUS_NAME,
            Gio.BusNameWatcherFlags.AUTO_START,
            () => {
                this.available = true;
                this.emit('availability', true);
                this._readLimits();
            },
            () => {
                this.available = false;
                this.emit('availability', false);
            });
    }

    async _readLimits() {
        try {
            const reply = await this._bus.call(BUS_NAME, OBJECT_PATH, 'org.freedesktop.DBus.Properties',
                'Get', new GLib.Variant('(ss)', [IFACE, 'MaxClipBytes']), new GLib.VariantType('(v)'),
                Gio.DBusCallFlags.NONE, -1, this._cancellable);
            const v = reply.recursiveUnpack()[0];
            if (typeof v === 'number' && v > 0)
                this.maxClipBytes = v;
        } catch (e) {
            console.debug(`msi-center: clipd MaxClipBytes: ${e.message}`);
        }
    }

    async _call(method, params, replyType) {
        try {
            return await this._bus.call(BUS_NAME, OBJECT_PATH, IFACE, method, params,
                replyType ? new GLib.VariantType(replyType) : null,
                Gio.DBusCallFlags.NONE, -1, this._cancellable);
        } catch (e) {
            const dbusName = Gio.DBusError.get_remote_error(e) ?? '';
            if (dbusName)
                Gio.DBusError.strip_remote_error(e);
            const err = new Error(e.message);
            err.dbusName = dbusName;
            throw err;
        }
    }

    /**
     * @param {string} mime
     * @param {GLib.Bytes} bytes
     * @returns {Promise<number>} clip id
     */
    async add(mime, bytes) {
        const params = GLib.Variant.new_tuple([
            GLib.Variant.new_string(mime),
            GLib.Variant.new_from_bytes(new GLib.VariantType('ay'), bytes, true),
        ]);
        const reply = await this._call('Add', params, '(t)');
        return reply.deepUnpack()[0];
    }

    /**
     * @returns {Promise<{total: number, clips: Array<object>}>}
     */
    async list(query, tags, scope, offset, limit) {
        const reply = await this._call('List',
            new GLib.Variant('(sassuu)', [query, tags, scope, offset, limit]), '(ua(tsstxxbas))');
        const [total, raw] = reply.deepUnpack();
        const clips = raw.map(([id, mime, preview, size, createdMs, lastUsedMs, pinned, clipTags]) => ({
            id, mime, preview, size: Number(size), createdMs: Number(createdMs),
            lastUsedMs: Number(lastUsedMs), pinned, tags: clipTags,
        }));
        return {total, clips};
    }

    /** @returns {Promise<{mime: string, bytes: GLib.Bytes}>} */
    async get(id) {
        const reply = await this._call('Get', new GLib.Variant('(t)', [id]), '(say)');
        const mime = reply.get_child_value(0).get_string()[0];
        const bytes = reply.get_child_value(1).get_data_as_bytes();
        return {mime, bytes};
    }

    pin(id) {
        return this._call('Pin', new GLib.Variant('(t)', [id]), null);
    }

    unpin(id) {
        return this._call('Unpin', new GLib.Variant('(t)', [id]), null);
    }

    async delete(ids) {
        const reply = await this._call('Delete', new GLib.Variant('(at)', [ids]), '(u)');
        return reply.deepUnpack()[0];
    }

    async setTags(id, tags) {
        const reply = await this._call('SetTags', new GLib.Variant('(tas)', [id, tags]), '(as)');
        return reply.deepUnpack()[0];
    }

    /** @returns {Promise<Array<[string, number]>>} */
    async tags() {
        const reply = await this._call('Tags', null, '(a(su))');
        return reply.deepUnpack()[0];
    }

    destroy() {
        this._cancellable.cancel();
        if (this._watchId)
            Gio.bus_unwatch_name(this._watchId);
        this._watchId = 0;
        if (this._sub)
            this._bus.signal_unsubscribe(this._sub);
        this._sub = 0;
        this.disconnectAll();
    }
}
