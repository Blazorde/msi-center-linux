// SPDX-License-Identifier: GPL-3.0-or-later
// Pure presentation helpers: no `gi://` imports so they run under Node (`scripts/ext-check.sh`).
// No business logic: values come from msi-daemon / clipd as-is, we only turn them into short strings.

/** Metric ids, in default order. Must match the `metric-order` default in the GSettings schema. */
export const METRICS = ['cpu', 'dgpu', 'igpu', 'ram', 'npu', 'cpu-temp', 'gpu-temp', 'fan'];

/** Human labels (French UI). */
export const METRIC_LABELS = {
    'cpu': 'CPU',
    'dgpu': 'dGPU',
    'igpu': 'iGPU',
    'ram': 'RAM',
    'npu': 'NPU',
    'cpu-temp': 'Température CPU',
    'gpu-temp': 'Température GPU',
    'fan': 'Ventilateurs',
};

/**
 * Profile ids exposed by `org.blazorde.MsiCenter1.SetProfile`, ordered from quietest to fastest, with their UI
 * label, a one-line caption and the Adwaita symbolic glyph (the same ones GNOME uses for power profiles).
 */
export const PROFILES = [
    {id: 'eco', label: 'Éco', caption: 'Silence · autonomie', icon: 'power-profile-power-saver-symbolic'},
    {id: 'balanced', label: 'Équilibré', caption: 'Usage courant', icon: 'power-profile-balanced-symbolic'},
    {id: 'gaming', label: 'Gaming', caption: 'Performances max', icon: 'power-profile-performance-symbolic'},
];

/**
 * @param {string} id profile id
 * @returns {string} label, or the raw id when unknown
 */
export function profileLabel(id) {
    return PROFILES.find(p => p.id === id)?.label ?? (id || '–');
}

/**
 * @param {string} id profile id
 * @returns {{id: string, label: string, caption: string, icon: string}|null}
 */
export function profileInfo(id) {
    return PROFILES.find(p => p.id === id) ?? null;
}

/**
 * Fan profiles (docs/hardware/fan-safety.md, « Profils de ventilation »). `short` fits a compact menu chip;
 * `label` is the full French name (accessibility, status line).
 */
export const FAN_PROFILES = [
    {id: 'silent', label: 'Silencieux', short: 'Silence', icon: 'weather-clear-night-symbolic'},
    {id: 'auto', label: 'Auto', short: 'Auto', icon: 'emblem-system-symbolic'},
    {id: 'balanced', label: 'Équilibré', short: 'Équilibré', icon: 'weather-windy-symbolic'},
    {id: 'turbo', label: 'Turbo', short: 'Turbo', icon: 'weather-tornado-symbolic'},
    {id: 'custom', label: 'Personnalisé', short: 'Perso', icon: 'document-edit-symbolic'},
];

/**
 * @param {string} id fan profile id
 * @returns {{id: string, label: string, short: string, icon: string}|null}
 */
export function fanProfileInfo(id) {
    return FAN_PROFILES.find(p => p.id === id) ?? null;
}

/**
 * Status line fragment for the fans: the fan profile when the daemon reports it, else the raw fan mode.
 *
 * @param {string|undefined} fanProfile `FanProfile` property (absent with an older daemon)
 * @param {string|undefined} fanMode `FanMode` property
 * @returns {string} French fragment, empty when nothing is known
 */
export function fanStatusText(fanProfile, fanMode) {
    const info = fanProfileInfo(fanProfile ?? '');
    if (info)
        return info.id === 'auto' ? 'ventilation Auto (courbe d’usine)' : `ventilation ${info.label}`;
    if (!fanMode)
        return '';
    return fanMode === 'advanced' ? 'courbes actives' : `ventilos : ${fanMode}`;
}

/**
 * Next profile in quiet → fast order. Pure UI navigation: the daemon still validates the id.
 *
 * @param {string} current current profile id (unknown → starts from `balanced`)
 * @param {number} delta +1 = faster, -1 = quieter
 * @param {boolean} wrap true wraps around (shortcut), false clamps at the ends (scroll)
 * @returns {string} profile id (equal to `current` when clamped)
 */
export function cycleProfile(current, delta, wrap = true) {
    const n = PROFILES.length;
    let i = PROFILES.findIndex(p => p.id === current);
    if (i < 0)
        i = PROFILES.findIndex(p => p.id === 'balanced');
    const step = Math.sign(delta);
    const j = wrap ? (((i + step) % n) + n) % n : Math.min(n - 1, Math.max(0, i + step));
    return PROFILES[j].id;
}

/** Top bar boxes accepted by `Main.panel.addToStatusArea`. */
export const PANEL_BOXES = ['left', 'center', 'right'];

/**
 * Sanitize a top bar placement read from GSettings.
 *
 * @param {string} box `left` · `center` · `right`
 * @param {number} index position inside the box (0 = first)
 * @param {string} fallback box used when `box` is unknown
 * @returns {{box: string, index: number}}
 */
export function panelPlacement(box, index, fallback = 'right') {
    const b = PANEL_BOXES.includes(box) ? box : fallback;
    const i = Number.isInteger(index) && index >= 0 ? index : 0;
    return {box: b, index: i};
}

/**
 * GTK accelerator → short French hint, e.g. `<Super><Shift>v` → `Super+Maj+V`.
 *
 * @param {string} accel
 * @returns {string}
 */
export function formatAccel(accel) {
    return (accel ?? '').replace(/<Super>/gi, 'Super+').replace(/<Control>|<Primary>|<Ctrl>/gi, 'Ctrl+')
        .replace(/<Shift>/gi, 'Maj+').replace(/<Alt>/gi, 'Alt+')
        .replace(/(^|\+)([a-z])$/, (_m, sep, c) => `${sep}${c.toUpperCase()}`);
}

/**
 * Unobtrusive badge text for a count of clips.
 *
 * @param {number} n
 * @returns {string} '' when nothing to show, `99+` above 99
 */
export function badgeText(n) {
    if (!Number.isFinite(n) || n <= 0)
        return '';
    return n > 99 ? '99+' : String(Math.floor(n));
}

const pct = v => (typeof v === 'number' && Number.isFinite(v) ? `${Math.round(v)}%` : null);
const deg = v => (typeof v === 'number' && Number.isFinite(v) && v > 0 ? `${Math.round(v)}°` : null);

/**
 * Normalize a metric order coming from GSettings: drop unknown/duplicate ids and append missing ones,
 * so a stale or hand-edited key never hides a metric from the preferences.
 *
 * @param {string[]} order
 * @returns {string[]}
 */
export function normalizeOrder(order) {
    const seen = new Set();
    const out = [];
    for (const id of order ?? []) {
        if (METRICS.includes(id) && !seen.has(id)) {
            seen.add(id);
            out.push(id);
        }
    }
    for (const id of METRICS) {
        if (!seen.has(id))
            out.push(id);
    }
    return out;
}

/**
 * Format one metric from a `Telemetry` dictionary (keys absent when unknown, see dbus-api.md).
 *
 * @param {string} id metric id (see METRICS)
 * @param {object} t telemetry, plain JS object (recursively unpacked a{sv})
 * @param {string} gpuState `GpuState` property (`active` · `suspended` · `removed`)
 * @returns {{prefix: string, value: string}|null} null when nothing to show
 */
export function formatMetric(id, t, gpuState = '') {
    t ??= {};
    switch (id) {
    case 'cpu':
        return wrap('CPU', pct(t.cpu_pct));
    case 'igpu':
        return wrap('iGPU', pct(t.igpu_pct));
    case 'ram':
        return wrap('RAM', pct(t.ram_pct));
    case 'npu':
        return wrap('NPU', pct(t.npu_pct));
    case 'dgpu': {
        const state = t.dgpu_state ?? gpuState;
        if (state === 'suspended')
            return {prefix: 'dGPU', value: 'veille'};
        if (state === 'removed')
            return {prefix: 'dGPU', value: 'off'};
        // Active without a load figure (NVML is only read in Gaming, never waking the GPU): say it is on.
        return wrap('dGPU', pct(t.dgpu_pct) ?? (state === 'active' ? 'on' : null));
    }
    case 'cpu-temp':
        return wrap('CPU', deg(t.cpu_temp) && `${deg(t.cpu_temp)}C`);
    case 'gpu-temp':
        return wrap('GPU', deg(t.gpu_temp) && `${deg(t.gpu_temp)}C`);
    case 'fan':
        return wrap('', formatFans(t.fan1_rpm, t.fan2_rpm));
    default:
        return null;
    }
}

function wrap(prefix, value) {
    return value === null || value === undefined ? null : {prefix, value};
}

/**
 * @param {number|undefined} fan1 rpm
 * @param {number|undefined} fan2 rpm
 * @returns {string|null} e.g. `2400·2100 tr/min`
 */
export function formatFans(fan1, fan2) {
    const ok = v => typeof v === 'number' && Number.isFinite(v);
    const parts = [fan1, fan2].filter(ok).map(v => `${Math.round(v)}`);
    if (parts.length === 0)
        return null;
    return `${parts.join('·')} tr/min`;
}

/**
 * @param {number} bytes
 * @returns {string} French binary units, e.g. `3,4 Kio`
 */
export function formatSize(bytes) {
    if (!Number.isFinite(bytes) || bytes < 0)
        return '';
    if (bytes < 1024)
        return `${bytes} o`;
    const units = ['Kio', 'Mio', 'Gio'];
    let v = bytes / 1024;
    let i = 0;
    while (v >= 1024 && i < units.length - 1) {
        v /= 1024;
        i++;
    }
    const s = v >= 10 ? Math.round(v).toString() : v.toFixed(1).replace('.', ',');
    return `${s} ${units[i]}`;
}

/**
 * @param {number} ms timestamp (ms since Unix epoch)
 * @param {number} now current time (ms), injectable for tests
 * @returns {string} French relative time
 */
export function formatRelative(ms, now = Date.now()) {
    const d = Math.max(0, now - ms);
    const min = 60_000;
    if (d < min)
        return 'à l’instant';
    if (d < 60 * min)
        return `il y a ${Math.floor(d / min)} min`;
    if (d < 24 * 60 * min)
        return `il y a ${Math.floor(d / (60 * min))} h`;
    if (d < 48 * 60 * min)
        return 'hier';
    const days = Math.floor(d / (24 * 60 * min));
    if (days < 30)
        return `il y a ${days} j`;
    const date = new Date(ms);
    const dd = String(date.getDate()).padStart(2, '0');
    const mm = String(date.getMonth() + 1).padStart(2, '0');
    return `${dd}/${mm}/${date.getFullYear()}`;
}

/**
 * Parse a user-typed tag list (`a, b c ,d`) into raw entries; clipd does the real normalization.
 *
 * @param {string} text
 * @returns {string[]}
 */
export function parseTags(text) {
    return (text ?? '').split(',').map(s => s.trim()).filter(s => s.length > 0);
}

/**
 * Pick which MIME type to capture from a selection offer. Text wins when a plain-text flavour exists
 * (office suites also offer a rendered image of copied cells); otherwise the best image.
 *
 * @param {string[]} mimes offered MIME types
 * @returns {{kind: 'text'|'image', mime: string}|null}
 */
export function pickCaptureMime(mimes) {
    const list = (mimes ?? []).map(m => String(m));
    const lower = list.map(m => m.toLowerCase());
    const textIdx = lower.findIndex(m => m === 'text/plain' || m.startsWith('text/plain;') ||
        m === 'utf8_string' || m === 'string' || m === 'text');
    if (textIdx >= 0)
        return {kind: 'text', mime: 'text/plain'};
    const pref = ['image/png', 'image/jpeg', 'image/webp', 'image/gif', 'image/bmp'];
    for (const p of pref) {
        const i = lower.indexOf(p);
        if (i >= 0)
            return {kind: 'image', mime: list[i]};
    }
    const img = list.find(m => m.toLowerCase().startsWith('image/'));
    if (img)
        return {kind: 'image', mime: img};
    return null;
}

/**
 * Bounded ring buffer for the sparkline.
 *
 * @param {number[]} history
 * @param {number} value
 * @param {number} max
 * @returns {number[]} same array, mutated
 */
export function pushSample(history, value, max = 60) {
    if (typeof value === 'number' && Number.isFinite(value)) {
        history.push(value);
        while (history.length > max)
            history.shift();
    }
    return history;
}

/** French text for msi-daemon D-Bus error names (`org.blazorde.MsiCenter1.Error.<X>`). */
export function daemonErrorText(name, message = '') {
    const short = (name ?? '').split('.').pop();
    switch (short) {
    case 'NotAuthorized':
        return 'Autorisation refusée (polkit).';
    case 'ReadOnly':
        return 'Mode lecture seule : EC non supporté, aucune écriture.';
    case 'SafetyFallback':
        return 'Écriture EC en échec : retour à la courbe usine (mode auto).';
    case 'GpuBusy':
        return 'Profil appliqué, mais le dGPU est utilisé : il reste allumé.';
    case 'InvalidArgument':
        return `Valeur refusée${message ? ` : ${message}` : '.'}`;
    case 'Hardware':
        return `Échec matériel${message ? ` : ${message}` : '.'}`;
    case 'ServiceUnknown':
    case 'NameHasNoOwner':
        return 'msi-daemon injoignable.';
    default:
        return message || 'Erreur inconnue.';
    }
}

/**
 * French text for a `SafetyEvent(s kind, d value, s detail)`.
 *
 * @returns {{level: 'info'|'warning'|'error', text: string}}
 */
export function safetyEventText(kind, value, detail = '') {
    const t = typeof value === 'number' && value > 0 ? ` (${Math.round(value)} °C)` : '';
    switch (kind) {
    case 'boost_forced':
        return {level: 'warning', text: `Cooler Boost forcé par la sécurité thermique${t}.`};
    case 'boost_released':
        return {level: 'info', text: `Cooler Boost relâché${t}.`};
    case 'readback_mismatch':
        return {level: 'error', text: 'Relecture EC divergente : retour au mode auto.'};
    case 'write_failed':
        return {level: 'error', text: 'Écriture EC en échec : retour au mode auto.'};
    case 'ec_reset':
        return {level: 'info', text: 'L’EC avait perdu les courbes : ré-appliquées.'};
    case 'gpu_busy':
        return {level: 'warning', text: 'dGPU occupé : il reste allumé.'};
    default:
        return {level: 'warning', text: detail || kind};
    }
}
