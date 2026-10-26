// SPDX-License-Identifier: GPL-3.0-or-later
// Mock of org.blazorde.MsiCenter1 (docs/architecture/dbus-api.md) for scripts/ext-smoke.sh.
// SAFETY: refuses to run unless DBUS_SYSTEM_BUS_ADDRESS points at a private bus under $MSI_SMOKE_DIR, so it
// can never be confused with (or race) the real msi-daemon. No hardware access whatsoever.
// Usage (inside the smoke harness only): gjs -m mock-msi-daemon.js   · env MOCK_READONLY=1 for S8 mode.
import Gio from 'gi://Gio';
import GLib from 'gi://GLib';

const addr = GLib.getenv('DBUS_SYSTEM_BUS_ADDRESS') ?? '';
const dir = GLib.getenv('MSI_SMOKE_DIR') ?? '';
if (!dir || !addr.includes(dir)) {
    printerr('mock-msi-daemon: refusing to run outside the private smoke bus (DBUS_SYSTEM_BUS_ADDRESS)');
    imports.system.exit(2);
}

const XML = `<node><interface name="org.blazorde.MsiCenter1">
  <method name="SetProfile"><arg type="s" direction="in"/></method>
  <method name="SetAutoProfileOnPower"><arg type="b" direction="in"/></method>
  <method name="SetCoolerBoost"><arg type="b" direction="in"/></method>
  <method name="SetChargeLimit"><arg type="y" direction="in"/></method>
  <method name="GpuHolders"><arg type="a(us)" direction="out"/></method>
  <signal name="Telemetry"><arg type="a{sv}"/></signal>
  <signal name="SafetyEvent"><arg type="s"/><arg type="d"/><arg type="s"/></signal>
  <property name="Profile" type="s" access="read"/>
  <property name="AutoProfileOnPower" type="b" access="read"/>
  <property name="OnAcPower" type="b" access="read"/>
  <property name="FanMode" type="s" access="read"/>
  <property name="CoolerBoost" type="b" access="read"/>
  <property name="BoostForced" type="b" access="read"/>
  <property name="ChargeLimit" type="y" access="read"/>
  <property name="GpuState" type="s" access="read"/>
  <property name="ReadOnly" type="b" access="read"/>
  <property name="ReadOnlyReason" type="s" access="read"/>
  <property name="FirmwareVersion" type="s" access="read"/>
</interface></node>`;

const readOnly = GLib.getenv('MOCK_READONLY') === '1';
const state = {
    Profile: 'balanced', AutoProfileOnPower: true, OnAcPower: true, FanMode: readOnly ? 'auto' : 'advanced',
    CoolerBoost: false, BoostForced: false, ChargeLimit: 80, GpuState: 'suspended',
    ReadOnly: readOnly, ReadOnlyReason: readOnly ? 'firmware 14K2EMS1.999 not supported (mock)' : '',
    FirmwareVersion: '14K2EMS1.110',
};
const TYPES = {
    Profile: 's', AutoProfileOnPower: 'b', OnAcPower: 'b', FanMode: 's', CoolerBoost: 'b', BoostForced: 'b',
    ChargeLimit: 'y', GpuState: 's', ReadOnly: 'b', ReadOnlyReason: 's', FirmwareVersion: 's',
};

let exported = null;
const set = (k, v) => {
    state[k] = v;
    exported.emit_property_changed(k, new GLib.Variant(TYPES[k], v));
};
const fail = (inv, name, msg) => inv.return_dbus_error(`org.blazorde.MsiCenter1.Error.${name}`, msg);
const guard = inv => {
    if (state.ReadOnly) {
        fail(inv, 'ReadOnly', 'EC not supported (mock)');
        return false;
    }
    return true;
};

const impl = {
    get Profile() { return state.Profile; },
    get AutoProfileOnPower() { return state.AutoProfileOnPower; },
    get OnAcPower() { return state.OnAcPower; },
    get FanMode() { return state.FanMode; },
    get CoolerBoost() { return state.CoolerBoost; },
    get BoostForced() { return state.BoostForced; },
    get ChargeLimit() { return state.ChargeLimit; },
    get GpuState() { return state.GpuState; },
    get ReadOnly() { return state.ReadOnly; },
    get ReadOnlyReason() { return state.ReadOnlyReason; },
    get FirmwareVersion() { return state.FirmwareVersion; },
    SetProfileAsync([p], inv) {
        if (!guard(inv))
            return;
        if (!['eco', 'balanced', 'gaming'].includes(p)) {
            fail(inv, 'InvalidArgument', `unknown profile ${p}`);
            return;
        }
        set('Profile', p);
        set('GpuState', p === 'gaming' ? 'active' : 'suspended');
        if (p === 'eco' && GLib.getenv('MOCK_GPU_BUSY') === '1') {
            fail(inv, 'GpuBusy', 'dGPU held by processes');
            return;
        }
        inv.return_value(null);
    },
    SetAutoProfileOnPowerAsync([on], inv) {
        if (guard(inv)) {
            set('AutoProfileOnPower', on);
            inv.return_value(null);
        }
    },
    SetCoolerBoostAsync([on], inv) {
        if (guard(inv)) {
            set('CoolerBoost', on);
            inv.return_value(null);
        }
    },
    SetChargeLimitAsync([v], inv) {
        if (!guard(inv))
            return;
        if (v < 50 || v > 100) {
            fail(inv, 'InvalidArgument', 'charge limit out of range');
            return;
        }
        set('ChargeLimit', v);
        inv.return_value(null);
    },
    GpuHolders() {
        return [[4242, 'mock-game']];
    },
};

exported = Gio.DBusExportedObject.wrapJSObject(XML, impl);
exported.export(Gio.DBus.system, '/org/blazorde/MsiCenter');
Gio.bus_own_name_on_connection(Gio.DBus.system, 'org.blazorde.MsiCenter1', Gio.BusNameOwnerFlags.NONE,
    () => print('mock-msi-daemon: serving on private bus'), () => {
        printerr('mock-msi-daemon: could not own name');
        imports.system.exit(1);
    });

let t = 0;
GLib.timeout_add(GLib.PRIORITY_DEFAULT, 1000, () => {
    t++;
    const v = (k, s) => new GLib.Variant(k, s);
    exported.emit_signal('Telemetry', new GLib.Variant('(a{sv})', [{
        cpu_pct: v('d', 20 + 15 * Math.sin(t / 3)), ram_pct: v('d', 41.2), igpu_pct: v('d', 7),
        npu_pct: v('d', 0), cpu_temp: v('d', 55 + 10 * Math.sin(t / 5)), gpu_temp: v('d', 0),
        fan1_rpm: v('u', 2400), fan2_rpm: v('u', 2100), fan1_pct: v('y', 40), fan2_pct: v('y', 35),
        dgpu_state: v('s', state.GpuState), cooler_boost: v('b', state.CoolerBoost),
    }]));
    if (t % 5 === 0) // periodic so late subscribers (the extension enables after ~6 s) see one
        exported.emit_signal('SafetyEvent', new GLib.Variant('(sds)', ['boost_forced', 92.0, 'mock event']));
    return GLib.SOURCE_CONTINUE;
});

new GLib.MainLoop(null, false).run();
