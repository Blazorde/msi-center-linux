// SPDX-License-Identifier: GPL-3.0-or-later
// Unit tests for the pure presentation helpers (run by scripts/ext-check.sh with `node --test`).
import assert from 'node:assert/strict';
import {test} from 'node:test';

import {
    METRICS, PROFILES, badgeText, cycleProfile, daemonErrorText, formatAccel, formatFans, formatMetric,
    formatRelative, formatSize, normalizeOrder, panelPlacement, parseTags, pickCaptureMime, profileInfo,
    FAN_PROFILES, fanProfileInfo, fanStatusText,
    profileLabel, pushSample, safetyEventText,
} from '../msi-center@blazorde/lib/format.js';

test('normalizeOrder drops unknown/duplicates and appends missing', () => {
    assert.deepEqual(normalizeOrder(['fan', 'bogus', 'fan', 'cpu']),
        ['fan', 'cpu', 'dgpu', 'igpu', 'ram', 'npu', 'cpu-temp', 'gpu-temp']);
    assert.deepEqual(normalizeOrder(undefined), METRICS);
});

test('formatMetric: percentages, temperatures, absent keys', () => {
    const t = {cpu_pct: 12.4, ram_pct: 41.6, cpu_temp: 61.7, fan1_rpm: 2400, fan2_rpm: 2100};
    assert.deepEqual(formatMetric('cpu', t), {prefix: 'CPU', value: '12%'});
    assert.deepEqual(formatMetric('ram', t), {prefix: 'RAM', value: '42%'});
    assert.deepEqual(formatMetric('cpu-temp', t), {prefix: 'CPU', value: '62°C'});
    assert.deepEqual(formatMetric('fan', t), {prefix: '', value: '2400·2100 tr/min'});
    assert.equal(formatMetric('npu', t), null);
    assert.equal(formatMetric('gpu-temp', {gpu_temp: 0}), null);
    assert.equal(formatMetric('nope', t), null);
});

test('formatMetric: dGPU state wins over percentage', () => {
    assert.deepEqual(formatMetric('dgpu', {dgpu_state: 'suspended'}), {prefix: 'dGPU', value: 'veille'});
    assert.deepEqual(formatMetric('dgpu', {}, 'removed'), {prefix: 'dGPU', value: 'off'});
    assert.deepEqual(formatMetric('dgpu', {dgpu_state: 'active', dgpu_pct: 30}), {prefix: 'dGPU', value: '30%'});
    assert.deepEqual(formatMetric('dgpu', {dgpu_state: 'active'}), {prefix: 'dGPU', value: 'on'});
    assert.equal(formatMetric('dgpu', {}), null);
});

test('formatFans', () => {
    assert.equal(formatFans(undefined, undefined), null);
    assert.equal(formatFans(0, undefined), '0 tr/min');
});

test('formatSize uses French binary units', () => {
    assert.equal(formatSize(12), '12 o');
    assert.equal(formatSize(3482), '3,4 Kio');
    assert.equal(formatSize(10 * 1024 * 1024), '10 Mio');
    assert.equal(formatSize(-1), '');
});

test('formatRelative', () => {
    const now = 1_000_000_000_000;
    assert.equal(formatRelative(now - 5_000, now), 'à l’instant');
    assert.equal(formatRelative(now - 5 * 60_000, now), 'il y a 5 min');
    assert.equal(formatRelative(now - 3 * 3_600_000, now), 'il y a 3 h');
    assert.equal(formatRelative(now - 30 * 3_600_000, now), 'hier');
    assert.equal(formatRelative(now - 3 * 86_400_000, now), 'il y a 3 j');
    assert.match(formatRelative(now - 90 * 86_400_000, now), /^\d\d\/\d\d\/\d{4}$/);
});

test('parseTags', () => {
    assert.deepEqual(parseTags(' a, b c ,, d '), ['a', 'b c', 'd']);
    assert.deepEqual(parseTags(''), []);
});

test('pickCaptureMime prefers text, then png', () => {
    assert.deepEqual(pickCaptureMime(['image/png', 'text/plain;charset=utf-8']), {kind: 'text', mime: 'text/plain'});
    assert.deepEqual(pickCaptureMime(['text/html', 'image/jpeg', 'image/png']), {kind: 'image', mime: 'image/png'});
    assert.deepEqual(pickCaptureMime(['image/x-foo']), {kind: 'image', mime: 'image/x-foo'});
    assert.deepEqual(pickCaptureMime(['UTF8_STRING']), {kind: 'text', mime: 'text/plain'});
    assert.equal(pickCaptureMime(['application/x-bar']), null);
    assert.equal(pickCaptureMime([]), null);
});

test('pushSample bounds history and skips non-numbers', () => {
    const h = [];
    for (let i = 0; i < 70; i++)
        pushSample(h, i, 60);
    pushSample(h, undefined, 60);
    assert.equal(h.length, 60);
    assert.equal(h[0], 10);
});

test('labels and error texts', () => {
    assert.equal(profileLabel('eco'), 'Éco');
    assert.equal(profileLabel('custom'), 'custom');
    assert.match(daemonErrorText('org.blazorde.MsiCenter1.Error.ReadOnly'), /lecture seule/i);
    assert.match(daemonErrorText('org.blazorde.MsiCenter1.Error.InvalidArgument', 'boost forced'), /boost forced/);
    assert.equal(daemonErrorText('', 'boom'), 'boom');
    assert.equal(safetyEventText('boost_forced', 92.4).level, 'warning');
    assert.match(safetyEventText('boost_forced', 92.4).text, /92 °C/);
    assert.equal(safetyEventText('write_failed', 0).level, 'error');
    assert.equal(safetyEventText('mystery', 0, 'detail').text, 'detail');
});

test('profiles: quiet → fast order, glyph and caption for each', () => {
    assert.deepEqual(PROFILES.map(p => p.id), ['eco', 'balanced', 'gaming']);
    for (const p of PROFILES)
        assert.ok(p.icon.endsWith('-symbolic') && p.caption.length > 0, p.id);
    assert.equal(profileInfo('gaming').label, 'Gaming');
    assert.equal(profileInfo('turbo'), null);
});

test('cycleProfile wraps for the shortcut, clamps for scroll', () => {
    assert.equal(cycleProfile('eco', 1), 'balanced');
    assert.equal(cycleProfile('gaming', 1), 'eco');
    assert.equal(cycleProfile('eco', -1), 'gaming');
    assert.equal(cycleProfile('gaming', 1, false), 'gaming');
    assert.equal(cycleProfile('eco', -1, false), 'eco');
    assert.equal(cycleProfile('balanced', -3, false), 'eco');
    assert.equal(cycleProfile('', 1), 'gaming'); // unknown starts from balanced
});

test('panelPlacement sanitizes box and index', () => {
    assert.deepEqual(panelPlacement('left', 1), {box: 'left', index: 1});
    assert.deepEqual(panelPlacement('top', -2, 'left'), {box: 'left', index: 0});
    assert.deepEqual(panelPlacement('center', 2.5), {box: 'center', index: 0});
});

test('formatAccel produces short French hints', () => {
    assert.equal(formatAccel('<Super>v'), 'Super+V');
    assert.equal(formatAccel('<Super><Shift>p'), 'Super+Maj+P');
    assert.equal(formatAccel('<Primary><Alt>F5'), 'Ctrl+Alt+F5');
    assert.equal(formatAccel(''), '');
});

test('badgeText hides zero and caps at 99+', () => {
    assert.equal(badgeText(0), '');
    assert.equal(badgeText(NaN), '');
    assert.equal(badgeText(7), '7');
    assert.equal(badgeText(250), '99+');
});

test('fan profiles: stable ids, French labels, status text', () => {
    assert.deepEqual(FAN_PROFILES.map(p => p.id), ['silent', 'auto', 'balanced', 'turbo', 'custom']);
    assert.equal(fanProfileInfo('custom').label, 'Personnalisé');
    assert.equal(fanProfileInfo('max'), null);
    for (const p of FAN_PROFILES)
        assert.match(p.icon, /-symbolic$/);
    assert.equal(fanStatusText('turbo', 'advanced'), 'ventilation Turbo');
    assert.equal(fanStatusText('auto', 'auto'), 'ventilation Auto (courbe d’usine)');
    // Older daemon without FanProfile: the fan mode as before.
    assert.equal(fanStatusText(undefined, 'advanced'), 'courbes actives');
    assert.equal(fanStatusText(undefined, 'auto'), 'ventilos : auto');
    assert.equal(fanStatusText(undefined, undefined), '');
});
