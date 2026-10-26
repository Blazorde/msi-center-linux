// SPDX-License-Identifier: GPL-3.0-or-later
// Super+V clipboard history popup. View over clipd: List / Get / Pin / Unpin / Delete / SetTags / Tags.
// Keyboard-first: focus stays in the search field, keys are intercepted in the capture phase.
import Clutter from 'gi://Clutter';
import GLib from 'gi://GLib';
import Pango from 'gi://Pango';
import Shell from 'gi://Shell';
import St from 'gi://St';

import * as Main from 'resource:///org/gnome/shell/ui/main.js';
import {ensureActorVisibleInScrollView} from 'resource:///org/gnome/shell/misc/animationUtils.js';

import {formatRelative, formatSize, parseTags} from './format.js';
import {ThumbCache} from './thumbs.js';

const LIST_LIMIT = 200;
const SEARCH_DEBOUNCE_MS = 120;
const OPEN_MS = 170;
const CLOSE_MS = 120;
const PASTE_DELAY_MS = 180; // let the previously focused window regain focus before Ctrl+V

export class ClipPopup {
    /**
     * @param {object} deps
     * @param {import('./clipd.js').ClipdClient} deps.clipd
     * @param {Gio.Settings} deps.settings
     * @param {import('./theme.js').ThemeWatcher} deps.theme
     */
    constructor({clipd, settings, theme}) {
        this._clipd = clipd;
        this._settings = settings;
        this._theme = theme;
        this._thumbs = new ThumbCache(clipd, settings);
        this._clips = [];
        this._index = 0;
        this._selected = new Set();
        this._activeTags = new Set();
        this._scope = 'all';
        this._seq = 0;
        this._searchTimeout = 0;
        this._pasteTimeout = 0;
        this._grab = null;
        this._root = null;
        this._clipd.connectObject('changed', () => {
            if (this.isOpen)
                this._scheduleReload(60);
        }, this);
    }

    get isOpen() {
        return this._grab !== null;
    }

    toggle() {
        if (this.isOpen)
            this.close();
        else
            this.open();
    }

    // ── Build ──────────────────────────────────────────────────────────────

    _build() {
        // Fixed layout: the card is positioned explicitly (centered, upper third), BinLayout alignment
        // proved unreliable for a card whose height changes with the list.
        this._root = new St.Widget({
            reactive: true,
            visible: false,
            style_class: 'msi-clip-root',
        });
        this._theme.track(this._root);

        this._backdrop = new St.Widget({style_class: 'msi-clip-backdrop', reactive: true});
        const click = new Clutter.ClickGesture();
        click.connect('recognize', () => this.close());
        this._backdrop.add_action(click);
        this._root.add_child(this._backdrop);

        this._card = new St.BoxLayout({
            orientation: Clutter.Orientation.VERTICAL,
            style_class: 'msi-glass msi-clip-card',
            reactive: true,
        });
        this._card.connect('notify::width', () => this._placeCard());
        this._theme.track(this._card); // `.msi-light.msi-glass` must match on the card itself
        this._root.add_child(this._card);

        // Header: search + scope.
        const header = new St.BoxLayout({style_class: 'msi-clip-header'});
        this._search = new St.Entry({
            style_class: 'msi-search',
            hint_text: 'Rechercher dans le presse-papiers…',
            can_focus: true,
            x_expand: true,
            primary_icon: new St.Icon({icon_name: 'edit-find-symbolic', style_class: 'msi-search-icon'}),
        });
        this._search.clutter_text.connect('text-changed', () => this._scheduleReload(SEARCH_DEBOUNCE_MS));
        header.add_child(this._search);
        this._scopeButtons = new Map();
        const scopes = new St.BoxLayout({style_class: 'msi-segmented msi-scope'});
        for (const [id, label] of [['all', 'Tout'], ['pinned', 'Épinglés']]) {
            const b = new St.Button({label, style_class: 'msi-segment', can_focus: true});
            b.connect('clicked', () => this._setScope(id));
            scopes.add_child(b);
            this._scopeButtons.set(id, b);
        }
        header.add_child(scopes);
        this._card.add_child(header);

        // Tag chips (filter, AND).
        this._tagBox = new St.BoxLayout({style_class: 'msi-chips'});
        this._tagScroll = new St.ScrollView({
            style_class: 'msi-chips-scroll',
            hscrollbar_policy: St.PolicyType.AUTOMATIC,
            vscrollbar_policy: St.PolicyType.NEVER,
            overlay_scrollbars: true,
            child: this._tagBox,
        });
        this._card.add_child(this._tagScroll);

        // List.
        this._list = new St.BoxLayout({orientation: Clutter.Orientation.VERTICAL, style_class: 'msi-clip-list'});
        this._scroll = new St.ScrollView({
            style_class: 'msi-clip-scroll vfade',
            hscrollbar_policy: St.PolicyType.NEVER,
            vscrollbar_policy: St.PolicyType.AUTOMATIC,
            overlay_scrollbars: true,
            x_expand: true,
            y_expand: true,
            child: this._list,
        });
        this._card.add_child(this._scroll);

        this._empty = new St.Label({style_class: 'msi-clip-empty', x_align: Clutter.ActorAlign.CENTER});
        this._card.add_child(this._empty);

        // Inline tag editor (Ctrl+T).
        this._tagEntry = new St.Entry({
            style_class: 'msi-search msi-tag-entry',
            hint_text: 'Tags séparés par des virgules — Entrée pour valider (taguer épingle)',
            can_focus: true,
            visible: false,
        });
        this._card.add_child(this._tagEntry);

        // Footer.
        const footer = new St.BoxLayout({style_class: 'msi-clip-footer'});
        this._status = new St.Label({style_class: 'msi-clip-status', x_expand: true, y_align: Clutter.ActorAlign.CENTER});
        footer.add_child(this._status);
        footer.add_child(new St.Label({
            style_class: 'msi-clip-hints',
            y_align: Clutter.ActorAlign.CENTER,
            text: '↵ copier · ⇧↵ coller · Ctrl+P épingler · Ctrl+T tags · Ctrl+Espace sélection · Suppr supprimer · Échap',
        }));
        this._card.add_child(footer);

        const keys = new Clutter.KeyController();
        keys.connect('key-press', () => this._onKey(keys));
        this._root.add_action_full('msi-keys', Clutter.EventPhase.CAPTURE, keys);

        Main.layoutManager.uiGroup.add_child(this._root);
        global.focus_manager.add_group(this._card); // Tab / Shift+Tab between search, scope and tag chips
    }

    _syncBlur() {
        const want = this._settings.get_boolean('blur');
        const has = this._backdrop.get_effect('msi-blur');
        if (want && !has) {
            // Full-monitor background blur: the frosted card then needs no rounded-corner clipping.
            this._backdrop.add_effect_with_name('msi-blur', new Shell.BlurEffect({
                mode: Shell.BlurMode.BACKGROUND, radius: 36, brightness: 0.85,
            }));
        } else if (!want && has) {
            this._backdrop.remove_effect_by_name('msi-blur');
        }
    }

    // ── Open / close ───────────────────────────────────────────────────────

    open() {
        if (this.isOpen)
            return;
        if (!this._root)
            this._build();
        const mon = Main.layoutManager.currentMonitor;
        this._root.set_position(mon.x, mon.y);
        this._root.set_size(mon.width, mon.height);
        this._backdrop.set_size(mon.width, mon.height);
        this._card.style = `max-height: ${Math.round(mon.height * 0.72)}px;`;
        this._placeCard();
        this._syncBlur();
        Main.layoutManager.uiGroup.set_child_above_sibling(this._root, null);

        // GNOME 51: Clutter.Grab has no seat state any more; a revoked grab means another modal won.
        const grab = Main.pushModal(this._root, {actionMode: Shell.ActionMode.POPUP});
        if (grab.is_revoked()) {
            Main.popModal(grab);
            return;
        }
        this._grab = grab;

        this._selected.clear();
        this._index = 0;
        this._search.text = '';
        this._tagEntry.hide();
        this._syncScope();
        this._root.show();
        this._root.opacity = 0;
        this._card.set_pivot_point(0.5, 0);
        this._card.scale_x = this._card.scale_y = 0.96;
        this._root.ease({opacity: 255, duration: OPEN_MS, mode: Clutter.AnimationMode.EASE_OUT_QUAD});
        this._card.ease({scale_x: 1, scale_y: 1, duration: OPEN_MS + 60, mode: Clutter.AnimationMode.EASE_OUT_BACK});
        this._search.grab_key_focus();
        this._reload();
    }

    _placeCard() {
        const w = this._root.width;
        this._card.set_position(Math.round((w - this._card.width) / 2), Math.round(this._root.height * 0.12));
    }

    close() {
        if (!this.isOpen)
            return;
        Main.popModal(this._grab);
        this._grab = null;
        this._root.ease({
            opacity: 0,
            duration: CLOSE_MS,
            mode: Clutter.AnimationMode.EASE_IN_QUAD,
            onStopped: () => this._root?.hide(),
        });
        this._card.ease({scale_x: 0.98, scale_y: 0.98, duration: CLOSE_MS, mode: Clutter.AnimationMode.EASE_IN_QUAD});
    }

    // ── Data ───────────────────────────────────────────────────────────────

    _scheduleReload(ms) {
        if (this._searchTimeout)
            GLib.source_remove(this._searchTimeout);
        this._searchTimeout = GLib.timeout_add(GLib.PRIORITY_DEFAULT, ms, () => {
            this._searchTimeout = 0;
            this._reload();
            return GLib.SOURCE_REMOVE;
        });
    }

    async _reload() {
        const seq = ++this._seq;
        const keepId = this._clips[this._index]?.id;
        try {
            const [res, tags] = await Promise.all([
                this._clipd.list(this._search.text, [...this._activeTags], this._scope, 0, LIST_LIMIT),
                this._clipd.tags(),
            ]);
            if (seq !== this._seq || !this.isOpen)
                return;
            this._clips = res.clips;
            this._total = res.total;
            const known = new Set(this._clips.map(c => c.id));
            for (const id of this._selected) {
                if (!known.has(id))
                    this._selected.delete(id);
            }
            const kept = this._clips.findIndex(c => c.id === keepId);
            this._index = kept >= 0 ? kept : Math.min(this._index, Math.max(0, this._clips.length - 1));
            this._renderTags(tags);
            this._renderList();
            this._error = null;
        } catch (e) {
            if (seq !== this._seq)
                return;
            this._clips = [];
            this._error = e.dbusName?.endsWith('ServiceUnknown')
                ? 'clipd est injoignable (activation D-Bus en échec ?).'
                : `clipd : ${e.message}`;
            this._renderList();
        }
    }

    _renderTags(tags) {
        this._tagBox.destroy_all_children();
        for (const t of this._activeTags) {
            if (!tags.some(([name]) => name === t))
                this._activeTags.delete(t);
        }
        for (const [name, count] of tags) {
            const b = new St.Button({
                label: `${name} · ${count}`,
                style_class: 'msi-chip',
                can_focus: true,
                toggle_mode: true,
                checked: this._activeTags.has(name),
            });
            b.connect('clicked', () => {
                if (this._activeTags.has(name))
                    this._activeTags.delete(name);
                else
                    this._activeTags.add(name);
                this._reload();
            });
            this._tagBox.add_child(b);
        }
        this._tagScroll.visible = tags.length > 0;
    }

    _renderList() {
        this._list.destroy_all_children();
        this._rows = [];
        for (const [i, clip] of this._clips.entries()) {
            const row = this._buildRow(clip, i);
            this._list.add_child(row);
            this._rows.push(row);
        }
        if (this._error)
            this._empty.text = this._error;
        else if (this._search.text || this._activeTags.size)
            this._empty.text = 'Aucun résultat.';
        else
            this._empty.text = 'Historique vide. Copiez quelque chose !';
        this._empty.visible = this._clips.length === 0;
        this._scroll.visible = this._clips.length > 0;
        this._syncSelection();
    }

    _buildRow(clip, i) {
        const row = new St.Button({style_class: 'msi-clip-row', can_focus: false, x_expand: true, reactive: true});
        const box = new St.BoxLayout({style_class: 'msi-clip-row-box', x_expand: true});
        row.child = box;

        const check = new St.Icon({icon_name: 'object-select-symbolic', style_class: 'msi-check', y_align: Clutter.ActorAlign.CENTER});
        box.add_child(check);
        row._check = check;

        const body = new St.BoxLayout({orientation: Clutter.Orientation.VERTICAL, x_expand: true, style_class: 'msi-clip-body'});
        const isImage = clip.mime.startsWith('image/');
        if (isImage) {
            const thumb = new St.Widget({style_class: 'msi-thumb', x_align: Clutter.ActorAlign.START, content_gravity: Clutter.ContentGravity.RESIZE_ASPECT});
            const cached = this._thumbs.peek(clip.id);
            if (cached) {
                thumb.content = cached;
            } else {
                let alive = true;
                thumb.connect('destroy', () => {
                    alive = false;
                });
                this._thumbs.load(clip.id, clip.size).then(content => {
                    if (content && alive)
                        thumb.content = content;
                });
            }
            body.add_child(thumb);
        } else {
            const label = new St.Label({text: clip.preview || ' ', style_class: 'msi-clip-preview', x_expand: true});
            label.clutter_text.ellipsize = Pango.EllipsizeMode.END;
            body.add_child(label);
        }
        const meta = new St.BoxLayout({style_class: 'msi-clip-meta'});
        const kind = isImage ? `Image ${clip.mime.slice(6).toUpperCase()}` : 'Texte';
        meta.add_child(new St.Label({
            text: `${kind} · ${formatSize(clip.size)} · ${formatRelative(clip.lastUsedMs)}`,
            style_class: 'msi-clip-meta-text',
            y_align: Clutter.ActorAlign.CENTER,
        }));
        for (const t of clip.tags)
            meta.add_child(new St.Label({text: t, style_class: 'msi-chip msi-chip-small', y_align: Clutter.ActorAlign.CENTER}));
        body.add_child(meta);
        box.add_child(body);

        const pin = new St.Button({
            style_class: 'msi-icon-button msi-pin',
            child: new St.Icon({icon_name: 'view-pin-symbolic'}),
            toggle_mode: false,
            checked: clip.pinned,
            can_focus: false,
            y_align: Clutter.ActorAlign.CENTER,
            accessible_name: clip.pinned ? 'Désépingler' : 'Épingler',
        });
        pin.connect('clicked', () => this._togglePin(clip));
        box.add_child(pin);

        const del = new St.Button({
            style_class: 'msi-icon-button msi-delete',
            child: new St.Icon({icon_name: 'user-trash-symbolic'}),
            can_focus: false,
            y_align: Clutter.ActorAlign.CENTER,
            accessible_name: 'Supprimer',
        });
        del.connect('clicked', () => this._delete([clip.id]));
        box.add_child(del);

        row.connect('clicked', () => {
            const [, , mods] = global.get_pointer();
            if (mods & Clutter.ModifierType.CONTROL_MASK) {
                this._index = i;
                this._toggleSelected(clip.id);
            } else {
                this._index = i;
                this._activate(Boolean(mods & Clutter.ModifierType.SHIFT_MASK));
            }
        });
        row.connect('notify::hover', () => {
            if (row.hover && this._index !== i) {
                this._index = i;
                this._syncSelection(false);
            }
        });
        return row;
    }

    _syncSelection(scroll = true) {
        for (const [i, row] of (this._rows ?? []).entries()) {
            const clip = this._clips[i];
            if (i === this._index)
                row.add_style_pseudo_class('selected');
            else
                row.remove_style_pseudo_class('selected');
            if (this._selected.has(clip.id))
                row.add_style_pseudo_class('checked');
            else
                row.remove_style_pseudo_class('checked');
            row._check.visible = this._selected.size > 0;
        }
        if (scroll && this._rows?.[this._index]) {
            const row = this._rows[this._index];
            // Allocation may not exist yet right after a rebuild.
            GLib.idle_add(GLib.PRIORITY_DEFAULT, () => {
                if (row.get_stage())
                    ensureActorVisibleInScrollView(this._scroll, row);
                return GLib.SOURCE_REMOVE;
            });
        }
        const n = this._selected.size;
        const shown = this._clips.length;
        this._status.text = n > 0
            ? `${n} sélectionné${n > 1 ? 's' : ''} — Suppr pour supprimer`
            : `${shown}${this._total > shown ? ` / ${this._total}` : ''} clip${shown > 1 ? 's' : ''}`;
    }

    _setScope(id) {
        this._scope = id;
        this._syncScope();
        this._reload();
    }

    _syncScope() {
        for (const [id, b] of this._scopeButtons)
            b.checked = id === this._scope;
    }

    // ── Keyboard ───────────────────────────────────────────────────────────

    _onKey(keys) {
        const [, sym] = keys.get_key();
        const [, pressed] = keys.get_state();
        const ctrl = (pressed & Clutter.ModifierType.CONTROL_MASK) !== 0;
        const shift = (pressed & Clutter.ModifierType.SHIFT_MASK) !== 0;
        const focus = global.stage.key_focus;
        const inTagEntry = focus === this._tagEntry.clutter_text;
        const inSearch = focus === this._search.clutter_text;
        const n = this._clips.length;

        switch (sym) {
        case Clutter.KEY_Escape:
            if (this._tagEntry.visible) {
                this._tagEntry.hide();
                this._search.grab_key_focus();
            } else {
                this.close();
            }
            return Clutter.EVENT_STOP;
        case Clutter.KEY_Down:
        case Clutter.KEY_KP_Down:
            this._move(1);
            return Clutter.EVENT_STOP;
        case Clutter.KEY_Up:
        case Clutter.KEY_KP_Up:
            this._move(-1);
            return Clutter.EVENT_STOP;
        case Clutter.KEY_Page_Down:
            this._move(8);
            return Clutter.EVENT_STOP;
        case Clutter.KEY_Page_Up:
            this._move(-8);
            return Clutter.EVENT_STOP;
        case Clutter.KEY_Home:
        case Clutter.KEY_End:
            if (!ctrl)
                return Clutter.EVENT_PROPAGATE;
            this._index = sym === Clutter.KEY_Home ? 0 : Math.max(0, n - 1);
            this._syncSelection();
            return Clutter.EVENT_STOP;
        case Clutter.KEY_Return:
        case Clutter.KEY_KP_Enter:
        case Clutter.KEY_ISO_Enter:
            if (inTagEntry) {
                this._applyTags();
                return Clutter.EVENT_STOP;
            }
            if (!inSearch && focus && this._card.contains(focus))
                return Clutter.EVENT_PROPAGATE; // chips / scope buttons handle their own activation
            this._activate(shift);
            return Clutter.EVENT_STOP;
        case Clutter.KEY_Delete:
        case Clutter.KEY_KP_Delete:
            if (inTagEntry || (inSearch && this._search.text.length > 0 && !ctrl))
                return Clutter.EVENT_PROPAGATE;
            this._delete(this._selected.size > 0 ? [...this._selected] : this._currentIds());
            return Clutter.EVENT_STOP;
        default:
            break;
        }

        if (!ctrl)
            return Clutter.EVENT_PROPAGATE;
        switch (sym) {
        case Clutter.KEY_p:
        case Clutter.KEY_P:
            if (this._clips[this._index])
                this._togglePin(this._clips[this._index]);
            return Clutter.EVENT_STOP;
        case Clutter.KEY_t:
        case Clutter.KEY_T:
            this._openTagEditor();
            return Clutter.EVENT_STOP;
        case Clutter.KEY_space:
            if (this._clips[this._index])
                this._toggleSelected(this._clips[this._index].id);
            return Clutter.EVENT_STOP;
        case Clutter.KEY_a:
        case Clutter.KEY_A:
            if (inSearch && this._search.text.length > 0)
                return Clutter.EVENT_PROPAGATE; // keep text select-all while typing
            if (this._selected.size === n)
                this._selected.clear();
            else
                this._clips.forEach(c => this._selected.add(c.id));
            this._syncSelection(false);
            return Clutter.EVENT_STOP;
        case Clutter.KEY_f:
        case Clutter.KEY_F:
            this._search.grab_key_focus();
            return Clutter.EVENT_STOP;
        default:
            return Clutter.EVENT_PROPAGATE;
        }
    }

    _move(delta) {
        const n = this._clips.length;
        if (n === 0)
            return;
        this._index = Math.min(n - 1, Math.max(0, this._index + delta));
        this._syncSelection();
    }

    _currentIds() {
        const c = this._clips[this._index];
        return c ? [c.id] : [];
    }

    _toggleSelected(id) {
        if (this._selected.has(id))
            this._selected.delete(id);
        else
            this._selected.add(id);
        this._syncSelection(false);
    }

    // ── Actions ────────────────────────────────────────────────────────────

    async _activate(invertPaste) {
        const clip = this._clips[this._index];
        if (!clip)
            return;
        try {
            const {mime, bytes} = await this._clipd.get(clip.id);
            const cb = St.Clipboard.get_default();
            if (mime.startsWith('text/'))
                cb.set_text(St.ClipboardType.CLIPBOARD, new TextDecoder().decode(bytes.toArray()));
            else
                cb.set_content(St.ClipboardType.CLIPBOARD, mime, bytes);
        } catch (e) {
            this._error = `Copie impossible : ${e.message}`;
            this._status.text = this._error;
            return;
        }
        const paste = this._settings.get_boolean('paste-on-select') !== invertPaste;
        this.close();
        if (paste)
            this._schedulePaste();
    }

    _schedulePaste() {
        if (this._pasteTimeout)
            GLib.source_remove(this._pasteTimeout);
        this._pasteTimeout = GLib.timeout_add(GLib.PRIORITY_DEFAULT, PASTE_DELAY_MS, () => {
            this._pasteTimeout = 0;
            if (!this._vkbd) {
                const seat = global.stage.context.get_backend().get_default_seat();
                this._vkbd = seat.create_virtual_device(Clutter.InputDeviceType.KEYBOARD_DEVICE);
            }
            const t = GLib.get_monotonic_time();
            this._vkbd.notify_keyval(t, Clutter.KEY_Control_L, Clutter.KeyState.PRESSED);
            this._vkbd.notify_keyval(t + 1, Clutter.KEY_v, Clutter.KeyState.PRESSED);
            this._vkbd.notify_keyval(t + 2, Clutter.KEY_v, Clutter.KeyState.RELEASED);
            this._vkbd.notify_keyval(t + 3, Clutter.KEY_Control_L, Clutter.KeyState.RELEASED);
            return GLib.SOURCE_REMOVE;
        });
    }

    async _togglePin(clip) {
        try {
            if (clip.pinned)
                await this._clipd.unpin(clip.id);
            else
                await this._clipd.pin(clip.id);
        } catch (e) {
            this._status.text = `Épinglage impossible : ${e.message}`;
        }
        // clipd emits Changed → reload.
    }

    async _delete(ids) {
        if (ids.length === 0)
            return;
        try {
            await this._clipd.delete(ids);
            this._thumbs.forget(ids);
            for (const id of ids)
                this._selected.delete(id);
        } catch (e) {
            this._status.text = `Suppression impossible : ${e.message}`;
        }
    }

    _openTagEditor() {
        const clip = this._clips[this._index];
        if (!clip)
            return;
        this._tagTarget = clip.id;
        this._tagEntry.text = clip.tags.join(', ');
        this._tagEntry.show();
        this._tagEntry.grab_key_focus();
    }

    async _applyTags() {
        const id = this._tagTarget;
        this._tagEntry.hide();
        this._search.grab_key_focus();
        if (id === undefined)
            return;
        try {
            await this._clipd.setTags(id, parseTags(this._tagEntry.text));
        } catch (e) {
            this._status.text = e.dbusName?.endsWith('.InvalidArgument')
                ? `Tag invalide : ${e.message}` : `Tags impossibles : ${e.message}`;
        }
    }

    destroy() {
        if (this.isOpen) {
            Main.popModal(this._grab);
            this._grab = null;
        }
        for (const id of [this._searchTimeout, this._pasteTimeout]) {
            if (id)
                GLib.source_remove(id);
        }
        this._searchTimeout = this._pasteTimeout = 0;
        this._clipd.disconnectObject(this);
        this._thumbs.destroy();
        this._vkbd = null;
        if (this._card)
            global.focus_manager.remove_group(this._card);
        this._root?.destroy();
        this._root = null;
    }
}
