// SPDX-License-Identifier: GPL-3.0-or-later
// Scripted walk-through run INSIDE the isolated headless shell of scripts/ext-smoke.sh (never in the owner's
// session). Injected into a scratch copy of the extension; logs `SMOKE …` lines, `SMOKE FAIL …` on error.
import Gio from 'gi://Gio';
import GLib from 'gi://GLib';
import GdkPixbuf from 'gi://GdkPixbuf';
import Shell from 'gi://Shell';
import St from 'gi://St';

const log = msg => console.log(`SMOKE ${msg}`);
const sleep = ms => new Promise(r => GLib.timeout_add(GLib.PRIORITY_DEFAULT, ms, () => {
    r();
    return GLib.SOURCE_REMOVE;
}));
const check = (cond, what) => log(`${cond ? 'ok  ' : 'FAIL'} ${what}`);

async function shot(name) {
    const dir = GLib.getenv('MSI_SMOKE_SHOTS');
    if (!dir)
        return;
    const stream = Gio.File.new_for_path(`${dir}/${name}.png`).replace(null, false, 0, null);
    await new Shell.Screenshot().screenshot(false, stream);
    stream.close(null);
    log(`shot ${name}`);
}

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
    check(ind._profilePill.text === 'Équilibré', `profile pill = ${ind._profilePill.text}`);
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

    // Menu: profile switch through the mock
    ind.menu.open();
    await sleep(1200);
    await shot('menu');
    await ind._setProfile('gaming');
    await sleep(300);
    check(ind._profilePill.text === 'Gaming', `profile after SetProfile = ${ind._profilePill.text}`);
    check(ind._events.has('thermal'), 'SafetyEvent banner shown');
    ind.menu.close();

    // Popup
    pop.open();
    await sleep(1200);
    check(pop.isOpen && pop._clips.length === 6, `popup lists ${pop._clips.length} clips`);
    const thumbRow = pop._clips.findIndex(c => c.mime.startsWith('image/'));
    check(thumbRow >= 0 && pop._thumbs._total > 0, 'image thumbnail decoded');
    pop._move(1);
    await shot('popup');
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

    // Light variant
    new Gio.Settings({schema_id: 'org.gnome.desktop.interface'}).set_string('color-scheme', 'prefer-light');
    await sleep(500);
    ind.menu.open();
    await sleep(1200);
    await shot('menu-light');
    ind.menu.close();
    pop.open();
    await sleep(1200);
    await shot('popup-light');
    pop._selected.add(pop._clips[0].id);
    await pop._delete([...pop._selected]);
    await sleep(500);
    check(pop._clips.length === 5, `after delete: ${pop._clips.length}`);
    pop.close();

    // Lifecycle
    ext.disable();
    ext.enable();
    check(ext._indicator !== null, 'disable/enable cycle');
}
