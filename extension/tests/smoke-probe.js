// SPDX-License-Identifier: GPL-3.0-or-later
// Scripted walk-through run INSIDE the isolated headless shell of scripts/ext-smoke.sh (never in the owner's
// session). Injected into a scratch copy of the extension; logs `SMOKE …` lines, `SMOKE FAIL …` on error.
import Gio from 'gi://Gio';
import GLib from 'gi://GLib';
import Clutter from 'gi://Clutter';
import GdkPixbuf from 'gi://GdkPixbuf';
import Shell from 'gi://Shell';
import St from 'gi://St';

import * as Main from 'resource:///org/gnome/shell/ui/main.js';

const log = msg => console.log(`SMOKE ${msg}`);
const sleep = ms => new Promise(r => GLib.timeout_add(GLib.PRIORITY_DEFAULT, ms, () => {
    r();
    return GLib.SOURCE_REMOVE;
}));
const check = (cond, what) => log(`${cond ? 'ok  ' : 'FAIL'} ${what}`);

/** Screenshot of an area (stage coordinates), or of an actor's box plus a margin for its shadow. */
async function shot(name, area) {
    const dir = GLib.getenv('MSI_SMOKE_SHOTS');
    if (!dir)
        return;
    let [x, y, w, h] = Array.isArray(area) ? area : [0, 0, global.stage.width, global.stage.height];
    if (area && !Array.isArray(area)) {
        const m = 28;
        const [ax, ay] = area.get_transformed_position();
        const [aw, ah] = area.get_transformed_size();
        x = Math.max(0, ax - m);
        y = Math.max(0, ay - m);
        w = Math.min(global.stage.width - x, aw + 2 * m);
        h = Math.min(global.stage.height - y, ah + 2 * m);
    }
    const stream = Gio.File.new_for_path(`${dir}/${name}.png`).replace(null, false, 0, null);
    await new Shell.Screenshot().screenshot_area(Math.round(x), Math.round(y), Math.round(w), Math.round(h), stream);
    stream.close(null);
    log(`shot ${name}`);
}

const panelShots = async suffix => {
    const h = Main.panel.height;
    await shot(`panel-left${suffix}`, [0, 0, 560, h]);
    await shot(`panel-right${suffix}`, [global.stage.width - 360, 0, 360, h]);
};

export async function run(ext) {
    try {
        await walk(ext);
        log('DONE');
    } catch (e) {
        log(`FAIL exception ${e}\n${e.stack}`);
    }
}

async function walk(ext) {
    const ind = ext._indicator;
    const pop = ext._popup;
    const cb = St.Clipboard.get_default();
    await sleep(2500);
    check(ext._daemon.available, 'mock daemon visible');
    check(ind._chipLabel.text === 'Équilibré', `panel profile chip = ${ind._chipLabel.text}`);
    check(Main.panel._leftBox.contains(ind.container), 'main indicator in the LEFT box');
    check(Main.panel._leftBox.get_child_at_index(1) === ind.container, 'main indicator right after Activities');
    check(ext._clipButton && Main.panel._rightBox.contains(ext._clipButton.container), 'clipboard button in the RIGHT box');
    check(!ind._clipItem.visible, 'no clipboard entry in the menu while the button is shown');
    const metrics = [...ind._metricItems.values()].filter(i => i.box.visible).map(i => `${i.prefix.text} ${i.value.text}`);
    check(metrics.length > 0, `panel metrics: ${metrics.join(' | ')}`);

    // Clipboard capture → clipd
    for (const t of ['git commit -m "feat(ext): liquid glass"', 'https://fr.wikipedia.org/wiki/Golden_Gate_Bridge',
        'Rendez-vous 18h30 au Crissy Field', 'cargo test --workspace --locked', 'International Orange #C0362C']) {
        cb.set_text(St.ClipboardType.CLIPBOARD, t);
        await sleep(250);
    }
    const pb = GdkPixbuf.Pixbuf.new(GdkPixbuf.Colorspace.RGB, true, 8, 320, 200);
    pb.fill(0xF28C38FF);
    const [, png] = pb.save_to_bufferv('png', [], []);
    cb.set_content(St.ClipboardType.CLIPBOARD, 'image/png', new GLib.Bytes(png));
    await sleep(800);
    const {clips} = await ext._clipd.list('', [], 'all', 0, 50);
    check(clips.length === 6, `clipd has ${clips.length} clips`);
    const link = clips.find(c => c.preview.startsWith('https'));
    await ext._clipd.setTags(link.id, ['liens', 'golden gate']);

    await sleep(800);
    check(ext._clipButton.badgeText === '5', `badge = session clips (tagged = pinned): ${ext._clipButton.badgeText}`);
    await panelShots('');

    // Menu: the profile pills come first; optimistic switch through the mock
    ind.menu.open();
    await sleep(1200);
    check(ind.menu.box.get_child_at_index(0).contains(ind._pills.get('eco').pill), 'profile pills are the first menu item');
    await shot('menu', ind.menu.box);
    const pending = ind._setProfile('gaming');
    check(ind._pills.get('gaming').pill.checked && ind._pending === 'gaming', 'optimistic highlight before the reply');
    await pending;
    await sleep(300);
    check(ind._chipLabel.text === 'Gaming' && ind._pills.get('gaming').pill.checked, `after SetProfile: chip ${ind._chipLabel.text}`);
    check(ind._events.has('thermal'), 'SafetyEvent banner shown');

    // Failure → highlight reverts, error banner
    const real = ind._daemon.setProfile;
    ind._daemon.setProfile = () => {
        const e = new Error('refused (smoke)');
        e.dbusName = 'org.blazorde.MsiCenter1.Error.NotAuthorized';
        return Promise.reject(e);
    };
    await ind._setProfile('eco');
    ind._daemon.setProfile = real;
    check(ind._pills.get('gaming').pill.checked && !ind._pills.get('eco').pill.checked && ind._error !== null,
        'failed SetProfile reverts the highlight and shows an error');
    ind._error = null;
    ind._sync();
    await sleep(200);
    await shot('menu-gaming', ind.menu.box);
    ind.menu.close();
    await sleep(300);

    // Scroll / shortcut path (clamped scroll, OSD when the menu is closed)
    ext._settings.set_boolean('scroll-cycles-profile', true);
    ind.cycleProfile(-1, false);
    await sleep(600);
    check(ind._daemon.props.Profile === 'balanced', `cycle down → ${ind._daemon.props.Profile}`);
    ext._settings.set_boolean('scroll-cycles-profile', false);

    // Popup
    pop.open();
    await sleep(1200);
    check(pop.isOpen && pop._clips.length === 6, `popup lists ${pop._clips.length} clips`);
    const thumbRow = pop._clips.findIndex(c => c.mime.startsWith('image/'));
    check(thumbRow >= 0 && pop._thumbs._total > 0, 'image thumbnail decoded');
    pop._move(1);
    await shot('popup', pop._card);
    pop._search.text = 'golden';
    await sleep(500);
    check(pop._clips.length === 1, `search 'golden' → ${pop._clips.length}`);
    pop._search.text = '';
    await sleep(400);
    pop._index = pop._clips.findIndex(c => c.preview.startsWith('cargo'));
    await pop._activate(false);
    await sleep(300);
    const text = await new Promise(r => cb.get_text(St.ClipboardType.CLIPBOARD, (_c, t) => r(t)));
    check(text === 'cargo test --workspace --locked' && !pop.isOpen, 'Enter copies back and closes');

    // Clipboard button → popup dropped under it
    pop.open({anchor: ext._clipButton});
    await sleep(1200);
    const [, by] = ext._clipButton.get_transformed_position();
    const [, cy] = pop._card.get_transformed_position();
    check(pop.isOpen && cy > by + ext._clipButton.height - 1, `anchored popup under the button (card y ${Math.round(cy)})`);
    const [cx] = pop._card.get_transformed_position();
    await shot('clipboard-anchored', [cx - 28, 0, global.stage.width - cx + 28, cy + pop._card.height + 28]);
    pop.close();
    await sleep(400);

    // Light variant
    new Gio.Settings({schema_id: 'org.gnome.desktop.interface'}).set_string('color-scheme', 'prefer-light');
    await sleep(500);
    ind.menu.open();
    await sleep(1200);
    await shot('menu-light', ind.menu.box);
    ind.menu.close();
    pop.open();
    await sleep(1200);
    await shot('popup-light', pop._card);
    pop._selected.add(pop._clips[0].id);
    await pop._delete([...pop._selected]);
    await sleep(500);
    check(pop._clips.length === 5, `after delete: ${pop._clips.length}`);
    pop.close();

    // Keyboard: arrows move between pills without leaving the menu, Enter applies
    const kbd = global.stage.context.get_backend().get_default_seat()
        .create_virtual_device(Clutter.InputDeviceType.KEYBOARD_DEVICE);
    const tap = async key => {
        const t = GLib.get_monotonic_time();
        kbd.notify_keyval(t, key, Clutter.KeyState.PRESSED);
        kbd.notify_keyval(t + 1, key, Clutter.KeyState.RELEASED);
        await sleep(250);
    };
    ind.menu.open();
    await sleep(600);
    ind._pills.get('balanced').pill.grab_key_focus();
    await tap(Clutter.KEY_Left);
    check(ind.menu.isOpen && global.stage.key_focus === ind._pills.get('eco').pill, '← moves focus to Éco inside the menu');
    await tap(Clutter.KEY_Return);
    await sleep(400);
    check(ind._daemon.props.Profile === 'eco', `Enter applies the focused pill → ${ind._daemon.props.Profile}`);
    ind.menu.close();
    await sleep(300);

    // Placement settings rebuild the indicators live
    ext._settings.set_boolean('show-clipboard-button', false);
    await sleep(200);
    check(ext._clipButton === null && ext._indicator._clipItem.visible, 'hidden button → menu entry comes back');
    ext._settings.set_boolean('show-clipboard-button', true);
    ext._settings.set_string('main-indicator-box', 'right');
    await sleep(200);
    check(Main.panel._rightBox.contains(ext._indicator.container), 'main indicator moved to the right');
    ext._settings.set_string('main-indicator-box', 'left');
    await sleep(200);

    // Lifecycle
    ext.disable();
    ext.enable();
    check(ext._indicator !== null, 'disable/enable cycle');
}
