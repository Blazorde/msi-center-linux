// SPDX-License-Identifier: GPL-3.0-or-later
// Client for `org.blazorde.MsiCenter1` (system bus, msi-daemon). Contract: docs/architecture/dbus-api.md.
// Push only: properties via PropertiesChanged, metrics via the `Telemetry` signal. No polling.
// Never D-Bus-activates the daemon (NO_AUTO_START everywhere): starting it rewrites the EC tables.
import Gio from 'gi://Gio';
import GLib from 'gi://GLib';

import {EventEmitter} from 'resource:///org/gnome/shell/misc/signals.js';

const BUS_NAME = 'org.blazorde.MsiCenter1';
const OBJECT_PATH = '/org/blazorde/MsiCenter';
const IFACE = 'org.blazorde.MsiCenter1';
const PROPS_IFACE = 'org.freedesktop.DBus.Properties';
const CALL_TIMEOUT_MS = 30_000; // SetProfile may wait for the dGPU step.

/**
 * Events:
 * - `availability` (bool) — daemon name appeared / vanished
 * - `properties` (object) — full property snapshot after any change
 * - `telemetry` (object) — one `Telemetry` sample, plain JS values
 * - `safety-event` (kind, value, detail)
 */
export class DaemonClient extends EventEmitter {
    constructor() {
        super();
        this._bus = Gio.DBus.system;
        this.available = false;
        this.props = {};
        this._cancellable = new Gio.Cancellable();
        this._subs = [
            this._subscribe(IFACE, 'Telemetry', params => {
                const [data] = params.recursiveUnpack();
                this.emit('telemetry', data ?? {});
            }),
            this._subscribe(IFACE, 'SafetyEvent', params => {
                const [kind, value, detail] = params.deepUnpack();
                this.emit('safety-event', kind, value, detail);
            }),
            this._subscribe(PROPS_IFACE, 'PropertiesChanged', params => {
                const [iface, changed, invalidated] = params.recursiveUnpack();
                if (iface !== IFACE)
                    return;
                Object.assign(this.props, changed);
                if (invalidated?.length)
                    this._refreshAll();
                else
                    this.emit('properties', this.props);
            }),
        ];
        // Never auto-start: msi-daemon writes the EC when it starts, and the owner may have stopped it on
        // purpose. We only follow its presence (name-owner watching) and reconnect when it comes back.
        this._watchId = Gio.bus_watch_name(Gio.BusType.SYSTEM, BUS_NAME,
            Gio.BusNameWatcherFlags.NONE,
            () => this._onAppeared(),
            () => this._onVanished());
    }

    _subscribe(iface, member, handler) {
        return this._bus.signal_subscribe(BUS_NAME, iface, member, OBJECT_PATH, null,
            Gio.DBusSignalFlags.NONE,
            (_conn, _sender, _path, _iface, _member, params) => {
                try {
                    handler(params);
                } catch (e) {
                    logError(e, `msi-center: ${member}`);
                }
            });
    }

    _onAppeared() {
        this.available = true;
        this.emit('availability', true);
        this._refreshAll();
    }

    _onVanished() {
        this.available = false;
        this.props = {};
        this.emit('availability', false);
    }

    async _refreshAll() {
        try {
            const reply = await this._bus.call(BUS_NAME, OBJECT_PATH, PROPS_IFACE, 'GetAll',
                new GLib.Variant('(s)', [IFACE]), new GLib.VariantType('(a{sv})'),
                Gio.DBusCallFlags.NO_AUTO_START, -1, this._cancellable);
            const [all] = reply.recursiveUnpack();
            this.props = all ?? {};
            this.emit('properties', this.props);
        } catch (e) {
            if (!e.matches?.(Gio.IOErrorEnum, Gio.IOErrorEnum.CANCELLED))
                console.warn(`msi-center: GetAll failed: ${e.message}`);
        }
    }

    /**
     * Call a method. Rejects with an Error carrying `dbusName` (e.g. `…Error.GpuBusy`) and a stripped message.
     *
     * @param {string} method
     * @param {GLib.Variant|null} params
     * @param {string|null} replyType
     */
    async call(method, params = null, replyType = null) {
        if (!this.available) {
            const err = new Error('msi-daemon is not running');
            err.dbusName = 'org.freedesktop.DBus.Error.ServiceUnknown';
            throw err;
        }
        try {
            return await this._bus.call(BUS_NAME, OBJECT_PATH, IFACE, method, params,
                replyType ? new GLib.VariantType(replyType) : null,
                Gio.DBusCallFlags.NO_AUTO_START, CALL_TIMEOUT_MS, this._cancellable);
        } catch (e) {
            const dbusName = Gio.DBusError.get_remote_error(e) ?? '';
            if (dbusName)
                Gio.DBusError.strip_remote_error(e);
            const err = new Error(e.message);
            err.dbusName = dbusName;
            throw err;
        }
    }

    setProfile(id) {
        return this.call('SetProfile', new GLib.Variant('(s)', [id]));
    }

    setAutoProfileOnPower(on) {
        return this.call('SetAutoProfileOnPower', new GLib.Variant('(b)', [on]));
    }

    setCoolerBoost(on) {
        return this.call('SetCoolerBoost', new GLib.Variant('(b)', [on]));
    }

    setChargeLimit(limit) {
        return this.call('SetChargeLimit', new GLib.Variant('(y)', [limit]));
    }

    /** @returns {Promise<Array<[number, string]>>} (pid, name) */
    async gpuHolders() {
        const reply = await this.call('GpuHolders', null, '(a(us))');
        return reply.deepUnpack()[0];
    }

    destroy() {
        this._cancellable.cancel();
        if (this._watchId)
            Gio.bus_unwatch_name(this._watchId);
        this._watchId = 0;
        for (const id of this._subs)
            this._bus.signal_unsubscribe(id);
        this._subs = [];
        this.disconnectAll();
    }
}
