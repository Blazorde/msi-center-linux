// SPDX-License-Identifier: GPL-3.0-or-later
// Preferences (Adw). Only edits this extension's GSettings; the one exception is the explicit, user-clicked
// "free Super+V" button, which removes <Super>v from GNOME's notification-list shortcut.
import Adw from 'gi://Adw';
import Gio from 'gi://Gio';
import Gtk from 'gi://Gtk';

import {ExtensionPreferences} from 'resource:///org/gnome/Shell/Extensions/js/extensions/prefs.js';

import {METRICS, METRIC_LABELS, normalizeOrder} from './lib/format.js';

const SHELL_KEYBINDINGS = 'org.gnome.shell.keybindings';
const MESSAGE_TRAY_KEY = 'toggle-message-tray';

export default class MsiCenterPrefs extends ExtensionPreferences {
    async fillPreferencesWindow(window) {
        const settings = this.getSettings();
        window.set_default_size(560, 720);
        window.add(this._barPage(settings));
        window.add(this._clipboardPage(settings));
    }

    _barPage(settings) {
        const page = new Adw.PreferencesPage({title: 'Barre supérieure', icon_name: 'utilities-system-monitor-symbolic'});

        const display = new Adw.PreferencesGroup({title: 'Affichage'});
        display.add(this._switchRow(settings, 'icon-only', 'Icône seule', 'Masque les métriques, garde l’icône'));
        display.add(this._switchRow(settings, 'show-sparkline', 'Mini-graphe dans le menu', 'CPU % et °C sur 60 s'));
        display.add(this._switchRow(settings, 'blur', 'Verre dépoli', 'Flou d’arrière-plan du menu et de la popup'));
        page.add(display);

        const metrics = new Adw.PreferencesGroup({
            title: 'Métriques',
            description: 'Cochez celles à afficher ; les flèches changent l’ordre. Valeurs poussées par msi-daemon (~1 Hz).',
        });
        page.add(metrics);

        let rows = [];
        const rebuild = () => {
            for (const r of rows)
                metrics.remove(r);
            rows = [];
            const order = normalizeOrder(settings.get_strv('metric-order'));
            order.forEach((id, i) => {
                const row = new Adw.SwitchRow({title: METRIC_LABELS[id] ?? id});
                settings.bind(`show-${id}`, row, 'active', Gio.SettingsBindFlags.DEFAULT);
                const up = new Gtk.Button({icon_name: 'go-up-symbolic', valign: Gtk.Align.CENTER, sensitive: i > 0,
                    css_classes: ['flat'], tooltip_text: 'Monter'});
                const down = new Gtk.Button({icon_name: 'go-down-symbolic', valign: Gtk.Align.CENTER,
                    sensitive: i < order.length - 1, css_classes: ['flat'], tooltip_text: 'Descendre'});
                const swap = (a, b) => {
                    const o = order.slice();
                    [o[a], o[b]] = [o[b], o[a]];
                    settings.set_strv('metric-order', o);
                };
                up.connect('clicked', () => swap(i, i - 1));
                down.connect('clicked', () => swap(i, i + 1));
                row.add_prefix(up);
                row.add_prefix(down);
                metrics.add(row);
                rows.push(row);
            });
        };
        rebuild();
        const id = settings.connect('changed::metric-order', rebuild);
        page.connect('destroy', () => settings.disconnect(id));

        const reset = new Gtk.Button({label: 'Ordre par défaut', halign: Gtk.Align.END, css_classes: ['flat']});
        reset.connect('clicked', () => settings.set_strv('metric-order', METRICS));
        metrics.set_header_suffix(reset);
        return page;
    }

    _clipboardPage(settings) {
        const page = new Adw.PreferencesPage({title: 'Presse-papiers', icon_name: 'edit-paste-symbolic'});

        const capture = new Adw.PreferencesGroup({
            title: 'Capture',
            description: 'Texte et images copiés (y compris depuis un gestionnaire de mots de passe) sont envoyés à clipd. Les non-épinglés sont effacés au logout.',
        });
        capture.add(this._switchRow(settings, 'clipboard-capture', 'Capturer le presse-papiers', null));
        capture.add(this._switchRow(settings, 'paste-on-select', 'Coller après sélection',
            'Entrée colle dans la fenêtre précédente (Ctrl+V simulé) ; Maj+Entrée fait l’inverse'));
        const cache = new Adw.SpinRow({
            title: 'Cache de vignettes',
            subtitle: 'Mio d’images décodées gardées en mémoire',
            adjustment: new Gtk.Adjustment({lower: 4, upper: 512, step_increment: 4, page_increment: 32}),
        });
        settings.bind('thumbnail-cache-mb', cache, 'value', Gio.SettingsBindFlags.DEFAULT);
        capture.add(cache);
        page.add(capture);

        const shortcut = new Adw.PreferencesGroup({title: 'Raccourci'});
        const entry = new Adw.EntryRow({
            title: 'Ouvrir l’historique (format GTK, ex. <Super>v)', use_markup: false, show_apply_button: true,
        });
        entry.text = settings.get_strv('clipboard-shortcut')[0] ?? '';
        entry.connect('apply', () => {
            const accel = entry.text.trim();
            const [ok] = Gtk.accelerator_parse(accel);
            if (accel === '' || ok) {
                settings.set_strv('clipboard-shortcut', accel === '' ? [] : [accel]);
                entry.remove_css_class('error');
            } else {
                entry.add_css_class('error');
            }
        });
        shortcut.add(entry);

        // GNOME binds <Super>v to the notification list by default; offer (never force) to free it.
        const shell = new Gio.Settings({schema_id: SHELL_KEYBINDINGS});
        const conflict = new Adw.ActionRow({
            title: 'Conflit avec la liste des notifications',
            subtitle: 'GNOME utilise aussi Super+V pour ouvrir les notifications (Super+M reste disponible).',
        });
        const free = new Gtk.Button({label: 'Libérer Super+V', valign: Gtk.Align.CENTER});
        free.connect('clicked', () => {
            const cur = shell.get_strv(MESSAGE_TRAY_KEY);
            shell.set_strv(MESSAGE_TRAY_KEY, cur.filter(a => a.toLowerCase() !== '<super>v'));
            syncConflict();
        });
        conflict.add_suffix(free);
        const syncConflict = () => {
            conflict.visible = shell.get_strv(MESSAGE_TRAY_KEY).some(a => a.toLowerCase() === '<super>v');
        };
        syncConflict();
        shortcut.add(conflict);
        page.add(shortcut);
        return page;
    }

    _switchRow(settings, key, title, subtitle) {
        const row = new Adw.SwitchRow({title});
        if (subtitle)
            row.subtitle = subtitle;
        settings.bind(key, row, 'active', Gio.SettingsBindFlags.DEFAULT);
        return row;
    }
}
