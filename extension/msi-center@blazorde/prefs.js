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

        const profiles = new Adw.PreferencesGroup({
            title: 'Profils',
            description: 'Un clic sur l’indicateur ouvre le sélecteur Éco / Équilibré / Gaming.',
        });
        profiles.add(this._switchRow(settings, 'scroll-cycles-profile', 'Molette sur l’indicateur',
            'Vers le haut : plus performant ; vers le bas : plus silencieux'));
        profiles.add(this._accelRow(settings, 'profile-cycle-shortcut',
            'Raccourci « profil suivant » (format GTK, ex. <Super>F5 ; vide = aucun)'));
        page.add(profiles);

        const place = new Adw.PreferencesGroup({
            title: 'Emplacement',
            description: 'Zone de la barre et position dans la zone (0 = premier élément).',
        });
        place.add(this._boxRow(settings, 'main-indicator-box', 'Indicateur principal'));
        place.add(this._indexRow(settings, 'main-indicator-index', 'Position de l’indicateur',
            'À gauche, 1 = juste après Activités'));
        place.add(this._switchRow(settings, 'show-clipboard-button', 'Bouton presse-papiers',
            'Raccourci vers l’historique ; masqué, l’entrée revient dans le menu'));
        const clipBox = this._boxRow(settings, 'clipboard-button-box', 'Zone du bouton presse-papiers');
        const clipIndex = this._indexRow(settings, 'clipboard-button-index', 'Position du bouton presse-papiers',
            'À droite, 0 = avant les icônes système');
        const badge = this._switchRow(settings, 'clipboard-badge', 'Pastille du bouton', 'Nombre de clips de la session');
        for (const row of [clipBox, clipIndex, badge]) {
            settings.bind('show-clipboard-button', row, 'sensitive', Gio.SettingsBindFlags.GET);
            place.add(row);
        }
        page.add(place);

        const display = new Adw.PreferencesGroup({title: 'Affichage'});
        display.add(this._switchRow(settings, 'icon-only', 'Icône seule', 'Masque le nom du profil et les métriques, garde l’icône'));
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
        shortcut.add(this._accelRow(settings, 'clipboard-shortcut', 'Ouvrir l’historique (format GTK, ex. <Super>v)'));

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

    /** Entry row editing a single-accelerator `as` key; empty = no shortcut, invalid = red row. */
    _accelRow(settings, key, title) {
        const entry = new Adw.EntryRow({title, use_markup: false, show_apply_button: true});
        entry.text = settings.get_strv(key)[0] ?? '';
        entry.connect('apply', () => {
            const accel = entry.text.trim();
            const [ok] = Gtk.accelerator_parse(accel);
            if (accel === '' || ok) {
                settings.set_strv(key, accel === '' ? [] : [accel]);
                entry.remove_css_class('error');
            } else {
                entry.add_css_class('error');
            }
        });
        return entry;
    }

    _boxRow(settings, key, title) {
        const ids = ['left', 'center', 'right'];
        const row = new Adw.ComboRow({title, model: Gtk.StringList.new(['Gauche', 'Centre', 'Droite'])});
        row.selected = Math.max(0, ids.indexOf(settings.get_string(key)));
        row.connect('notify::selected', () => {
            if (ids[row.selected] && ids[row.selected] !== settings.get_string(key))
                settings.set_string(key, ids[row.selected]);
        });
        const id = settings.connect(`changed::${key}`, () => {
            row.selected = Math.max(0, ids.indexOf(settings.get_string(key)));
        });
        row.connect('destroy', () => settings.disconnect(id));
        return row;
    }

    _indexRow(settings, key, title, subtitle) {
        const row = new Adw.SpinRow({
            title, subtitle,
            adjustment: new Gtk.Adjustment({lower: 0, upper: 20, step_increment: 1, page_increment: 5}),
        });
        settings.bind(key, row, 'value', Gio.SettingsBindFlags.DEFAULT);
        return row;
    }

    _switchRow(settings, key, title, subtitle) {
        const row = new Adw.SwitchRow({title});
        if (subtitle)
            row.subtitle = subtitle;
        settings.bind(key, row, 'active', Gio.SettingsBindFlags.DEFAULT);
        return row;
    }
}
