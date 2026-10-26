//! Presse-papiers: full clipd manager — search, tag filter, list/grid, thumbnails, pin, tags, multi-delete.

use std::cell::{Cell, RefCell};
use std::collections::{BTreeSet, HashMap, HashSet};
use std::rc::Rc;

use adw::prelude::*;
use gtk::{gdk, glib};
use msi_center::errors::Service;
use msi_center::format;
use msi_center::model::{ClipEntry, Scope, parse_tag_input};

use super::widgets::{Ctx, Debounce, Gate, hint, page_header};

const PAGE: u32 = 60;

#[derive(Debug, Default)]
struct State {
    query: String,
    tags: BTreeSet<String>,
    scope: Scope,
    grid: bool,
    selecting: bool,
    selected: HashSet<u64>,
    clips: Vec<ClipEntry>,
    total: u32,
    limit: u32,
    seq: u64,
    thumbs: HashMap<u64, gdk::Texture>,
}

/// The page.
pub struct Clipboard {
    /// Root widget.
    pub gate: Gate,
    ctx: Rc<Ctx>,
    state: RefCell<State>,
    search: gtk::SearchEntry,
    tag_box: adw::WrapBox,
    count: gtk::Label,
    selection_bar: gtk::Revealer,
    selection_label: gtk::Label,
    delete_selected: gtk::Button,
    select_toggle: gtk::ToggleButton,
    flow: gtk::FlowBox,
    more: gtk::Button,
    stack: gtk::Stack,
    empty: adw::StatusPage,
    debounce: Rc<Debounce>,
    loaded_once: Cell<bool>,
}

impl std::fmt::Debug for Clipboard {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Clipboard").finish_non_exhaustive()
    }
}

fn now_ms() -> i64 {
    glib::real_time() / 1000
}

fn icon_button(icon: &str, tooltip: &str) -> gtk::Button {
    let b = gtk::Button::builder()
        .icon_name(icon)
        .tooltip_text(tooltip)
        .css_classes(["flat", "circular"])
        .build();
    b.update_property(&[gtk::accessible::Property::Label(tooltip)]);
    b
}

impl Clipboard {
    /// Builds the page.
    #[allow(clippy::too_many_lines, reason = "widget tree")]
    pub fn new(ctx: &Rc<Ctx>) -> Rc<Self> {
        let content = gtk::Box::new(gtk::Orientation::Vertical, 12);
        content.set_margin_start(16);
        content.set_margin_end(16);
        content.set_margin_top(12);
        content.append(&page_header(
            "Presse-papiers",
            "Historique de session, éléments épinglés et tags",
        ));

        let search = gtk::SearchEntry::builder()
            .placeholder_text("Rechercher dans le texte…")
            .hexpand(true)
            .build();
        let scope = adw::ToggleGroup::builder()
            .homogeneous(true)
            .css_classes(["segmented"])
            .build();
        for (s, label) in [
            (Scope::All, "Tout"),
            (Scope::Pinned, "Épinglés"),
            (Scope::Unpinned, "Session"),
        ] {
            scope.add(adw::Toggle::builder().name(s.as_str()).label(label).build());
        }
        scope.set_active_name(Some("all"));
        let view = adw::ToggleGroup::new();
        view.add(
            adw::Toggle::builder()
                .name("list")
                .icon_name("view-list-symbolic")
                .tooltip("Liste")
                .build(),
        );
        view.add(
            adw::Toggle::builder()
                .name("grid")
                .icon_name("view-grid-symbolic")
                .tooltip("Grille")
                .build(),
        );
        view.set_active_name(Some("list"));
        let select_toggle = gtk::ToggleButton::builder()
            .icon_name("selection-mode-symbolic")
            .tooltip_text("Sélectionner plusieurs éléments")
            .build();
        select_toggle.update_property(&[gtk::accessible::Property::Label("Sélection multiple")]);
        let clear = gtk::Button::builder()
            .label("Effacer l'historique")
            .tooltip_text("Supprime tous les éléments non épinglés")
            .css_classes(["destructive-action"])
            .build();

        let bar = gtk::FlowBox::builder()
            .selection_mode(gtk::SelectionMode::None)
            .max_children_per_line(5)
            .column_spacing(8)
            .row_spacing(8)
            .build();
        let search_box = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        search_box.set_hexpand(true);
        search_box.set_size_request(260, -1);
        search_box.append(&search);
        bar.append(&search_box);
        bar.append(&scope);
        bar.append(&view);
        bar.append(&select_toggle);
        bar.append(&clear);
        content.append(&bar);

        let tag_box = adw::WrapBox::builder().child_spacing(6).line_spacing(6).build();
        content.append(&tag_box);

        let count = gtk::Label::builder()
            .xalign(0.0)
            .css_classes(["dim", "numeric"])
            .build();
        content.append(&count);
        content.append(&hint(
            "Astuce : ajouter un tag épingle automatiquement l'élément — il est alors conservé après la \
             déconnexion. Les éléments non épinglés (session) sont effacés à la fermeture de session.",
        ));

        let selection_label = gtk::Label::builder().hexpand(true).xalign(0.0).build();
        let delete_selected = gtk::Button::builder()
            .label("Supprimer")
            .css_classes(["destructive-action"])
            .sensitive(false)
            .build();
        let sel_all = gtk::Button::builder().label("Tout sélectionner").build();
        let sel_box = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        sel_box.add_css_class("glass");
        sel_box.add_css_class("compact");
        sel_box.append(&selection_label);
        sel_box.append(&sel_all);
        sel_box.append(&delete_selected);
        let selection_bar = gtk::Revealer::builder()
            .transition_type(gtk::RevealerTransitionType::SlideDown)
            .child(&sel_box)
            .build();
        content.append(&selection_bar);

        let flow = gtk::FlowBox::builder()
            .selection_mode(gtk::SelectionMode::None)
            .homogeneous(false)
            .min_children_per_line(1)
            .max_children_per_line(1)
            .column_spacing(12)
            .row_spacing(12)
            .valign(gtk::Align::Start)
            .build();
        let more = gtk::Button::builder()
            .label("Afficher plus")
            .halign(gtk::Align::Center)
            .css_classes(["pill"])
            .visible(false)
            .build();
        let list_box = gtk::Box::new(gtk::Orientation::Vertical, 12);
        list_box.append(&flow);
        list_box.append(&more);
        list_box.set_margin_bottom(24);
        let empty = adw::StatusPage::builder()
            .icon_name("edit-paste-symbolic")
            .title("Aucun élément")
            .description("Copiez du texte ou une image : l'extension GNOME l'ajoute ici.")
            .build();
        let stack = gtk::Stack::builder()
            .transition_type(gtk::StackTransitionType::Crossfade)
            .vexpand(true)
            .build();
        stack.add_named(&list_box, Some("list"));
        stack.add_named(&empty, Some("empty"));
        let clamp = adw::Clamp::builder().maximum_size(1200).child(&stack).build();
        let scroller = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vexpand(true)
            .child(&clamp)
            .build();
        clamp.set_margin_start(16);
        clamp.set_margin_end(16);
        let outer = gtk::Box::new(gtk::Orientation::Vertical, 12);
        let head_clamp = adw::Clamp::builder().maximum_size(1200).child(&content).build();
        outer.append(&head_clamp);
        outer.append(&scroller);

        let gate = Gate::new(ctx, Service::Clipd, &outer);
        let page = Rc::new(Self {
            gate,
            ctx: ctx.clone(),
            state: RefCell::new(State {
                limit: PAGE,
                ..State::default()
            }),
            search,
            tag_box,
            count,
            selection_bar,
            selection_label,
            delete_selected,
            select_toggle,
            flow,
            more,
            stack,
            empty,
            debounce: Rc::default(),
            loaded_once: Cell::new(false),
        });

        // Ctrl+F focuses the search.
        let keys = gtk::ShortcutController::new();
        keys.set_scope(gtk::ShortcutScope::Managed);
        let w = Rc::downgrade(&page);
        keys.add_shortcut(gtk::Shortcut::new(
            gtk::ShortcutTrigger::parse_string("<Control>f"),
            Some(gtk::CallbackAction::new(move |_, _| {
                if let Some(p) = w.upgrade() {
                    p.search.grab_focus();
                }
                glib::Propagation::Stop
            })),
        ));
        outer.add_controller(keys);

        let w = Rc::downgrade(&page);
        page.search.connect_search_changed(move |e| {
            if let Some(p) = w.upgrade() {
                p.state.borrow_mut().query = e.text().to_string();
                p.state.borrow_mut().limit = PAGE;
                p.reload_soon(220);
            }
        });
        let w = Rc::downgrade(&page);
        scope.connect_active_name_notify(move |g| {
            let (Some(p), Some(name)) = (w.upgrade(), g.active_name()) else {
                return;
            };
            p.state.borrow_mut().scope = match name.as_str() {
                "pinned" => Scope::Pinned,
                "unpinned" => Scope::Unpinned,
                _ => Scope::All,
            };
            p.reload();
        });
        let w = Rc::downgrade(&page);
        view.connect_active_name_notify(move |g| {
            let Some(p) = w.upgrade() else { return };
            let grid = g.active_name().is_some_and(|n| n == "grid");
            p.state.borrow_mut().grid = grid;
            p.flow.set_max_children_per_line(if grid { 4 } else { 1 });
            // Grid: equal tiles; list: each card as tall as its content.
            p.flow.set_homogeneous(grid);
            p.render();
        });
        let w = Rc::downgrade(&page);
        page.select_toggle.connect_toggled(move |t| {
            let Some(p) = w.upgrade() else { return };
            {
                let mut s = p.state.borrow_mut();
                s.selecting = t.is_active();
                s.selected.clear();
            }
            p.render();
        });
        let w = Rc::downgrade(&page);
        sel_all.connect_clicked(move |_| {
            let Some(p) = w.upgrade() else { return };
            {
                let mut s = p.state.borrow_mut();
                let ids: Vec<u64> = s.clips.iter().map(|c| c.id).collect();
                s.selected.extend(ids);
            }
            p.render();
        });
        let w = Rc::downgrade(&page);
        page.delete_selected.connect_clicked(move |b| {
            if let Some(p) = w.upgrade() {
                p.confirm_delete_selected(b);
            }
        });
        let w = Rc::downgrade(&page);
        clear.connect_clicked(move |b| {
            if let Some(p) = w.upgrade() {
                p.confirm_clear(b);
            }
        });
        let w = Rc::downgrade(&page);
        page.more.connect_clicked(move |_| {
            if let Some(p) = w.upgrade() {
                p.state.borrow_mut().limit += PAGE;
                p.reload();
            }
        });
        page
    }

    /// clipd answered.
    pub fn up(self: &Rc<Self>) {
        self.gate.up();
        self.reload();
    }

    /// Something changed in clipd.
    pub fn changed(self: &Rc<Self>) {
        self.reload_soon(150);
    }

    fn reload_soon(self: &Rc<Self>, ms: u64) {
        let w = Rc::downgrade(self);
        self.debounce.run(ms, move || {
            if let Some(p) = w.upgrade() {
                p.reload();
            }
        });
    }

    /// Reloads the list and the tags.
    pub fn reload(self: &Rc<Self>) {
        let (query, tags, scope, limit, seq) = {
            let mut s = self.state.borrow_mut();
            s.seq += 1;
            (
                s.query.clone(),
                s.tags.iter().cloned().collect::<Vec<_>>(),
                s.scope,
                s.limit,
                s.seq,
            )
        };
        let w = Rc::downgrade(self);
        let ctx = self.ctx.clone();
        Ctx::spawn(async move {
            let res = ctx
                .app
                .backend
                .clips(move |c| async move {
                    let all_tags = c.tags().await?;
                    let page = c.list(&query, &tags, scope, 0, limit).await?;
                    Ok((all_tags, page))
                })
                .await;
            let Some(p) = w.upgrade() else { return };
            if p.state.borrow().seq != seq {
                return;
            }
            match res {
                Ok((all_tags, page)) => {
                    {
                        let mut s = p.state.borrow_mut();
                        let known: BTreeSet<String> = all_tags.iter().map(|(t, _)| t.clone()).collect();
                        s.tags.retain(|t| known.contains(t));
                        let ids: HashSet<u64> = page.clips.iter().map(|c| c.id).collect();
                        s.selected.retain(|id| ids.contains(id));
                        s.thumbs.retain(|id, _| ids.contains(id));
                        s.clips = page.clips;
                        s.total = page.total;
                    }
                    p.loaded_once.set(true);
                    p.render_tags(&all_tags);
                    p.render();
                }
                Err(e) if !e.is_unavailable() => ctx.error(&e),
                Err(_) => {}
            }
        });
    }

    fn render_tags(self: &Rc<Self>, all: &[(String, u32)]) {
        self.tag_box.remove_all();
        if all.is_empty() {
            return;
        }
        self.tag_box.append(
            &gtk::Label::builder()
                .label("Filtrer par tag :")
                .css_classes(["dim"])
                .build(),
        );
        let active = self.state.borrow().tags.clone();
        for (tag, n) in all {
            let b = gtk::ToggleButton::builder()
                .label(format!("{tag} ({n})"))
                .active(active.contains(tag))
                .css_classes(["tag-chip"])
                .build();
            let w = Rc::downgrade(self);
            let tag = tag.clone();
            b.connect_toggled(move |b| {
                let Some(p) = w.upgrade() else { return };
                {
                    let mut s = p.state.borrow_mut();
                    if b.is_active() {
                        s.tags.insert(tag.clone());
                    } else {
                        s.tags.remove(&tag);
                    }
                }
                p.reload();
            });
            self.tag_box.append(&b);
        }
    }

    fn render(self: &Rc<Self>) {
        self.flow.remove_all();
        let s = self.state.borrow();
        let shown = s.clips.len();
        self.count.set_label(&if s.total == 0 {
            String::new()
        } else if u32::try_from(shown).is_ok_and(|n| n < s.total) {
            format!("{shown} affichés sur {}", s.total)
        } else {
            format!("{} élément(s)", s.total)
        });
        self.more
            .set_visible(u32::try_from(shown).is_ok_and(|n| n < s.total));
        self.selection_bar.set_reveal_child(s.selecting);
        self.selection_label
            .set_label(&format!("{} sélectionné(s)", s.selected.len()));
        self.delete_selected.set_sensitive(!s.selected.is_empty());
        if s.clips.is_empty() {
            let filtered = !s.query.is_empty() || !s.tags.is_empty() || s.scope != Scope::All;
            self.empty.set_title(if filtered {
                "Aucun résultat"
            } else {
                "Aucun élément"
            });
            self.empty.set_description(Some(if filtered {
                "Essayez une autre recherche ou retirez des filtres."
            } else {
                "Copiez du texte ou une image : l'extension GNOME l'ajoute ici."
            }));
            self.stack.set_visible_child_name("empty");
            return;
        }
        self.stack.set_visible_child_name("list");
        let clips = s.clips.clone();
        drop(s);
        for c in &clips {
            let card = self.card(c);
            self.flow.append(&card);
        }
    }

    #[allow(clippy::too_many_lines, reason = "one card")]
    fn card(self: &Rc<Self>, c: &ClipEntry) -> gtk::Widget {
        let s = self.state.borrow();
        let (grid, selecting, selected) = (s.grid, s.selecting, s.selected.contains(&c.id));
        let thumb = s.thumbs.get(&c.id).cloned();
        drop(s);

        let root = gtk::Box::new(gtk::Orientation::Vertical, 8);
        for cls in ["glass", "compact", "clip-card", "interactive"] {
            root.add_css_class(cls);
        }
        if selected {
            root.add_css_class("selected");
        }

        let head = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        if selecting {
            let check = gtk::CheckButton::builder().active(selected).build();
            check.update_property(&[gtk::accessible::Property::Label("Sélectionner")]);
            let w = Rc::downgrade(self);
            let id = c.id;
            check.connect_toggled(move |b| {
                let Some(p) = w.upgrade() else { return };
                {
                    let mut s = p.state.borrow_mut();
                    if b.is_active() {
                        s.selected.insert(id);
                    } else {
                        s.selected.remove(&id);
                    }
                }
                p.render();
            });
            head.append(&check);
        }
        head.append(&gtk::Image::from_icon_name(if c.is_image() {
            "image-x-generic-symbolic"
        } else {
            "text-x-generic-symbolic"
        }));
        head.append(
            &gtk::Label::builder()
                .label(c.kind_label())
                .css_classes(["card-title"])
                .build(),
        );
        head.append(
            &gtk::Label::builder()
                .label(format!(
                    "{} · {}",
                    format::ago(c.last_used_ms, now_ms()),
                    format::bytes(c.size)
                ))
                .css_classes(["dim", "caption", "numeric"])
                .hexpand(true)
                .xalign(1.0)
                .ellipsize(gtk::pango::EllipsizeMode::End)
                .build(),
        );
        if c.pinned {
            let pin = gtk::Image::from_icon_name("view-pin-symbolic");
            pin.set_tooltip_text(Some("Épinglé : conservé après la déconnexion"));
            pin.update_property(&[gtk::accessible::Property::Label("Épinglé")]);
            head.append(&pin);
        }
        root.append(&head);

        if c.is_image() {
            let pic = gtk::Picture::builder()
                .content_fit(gtk::ContentFit::Cover)
                .height_request(if grid { 140 } else { 96 })
                .css_classes(["clip-thumb"])
                .alternative_text(c.kind_label())
                .build();
            if let Some(t) = thumb {
                pic.set_paintable(Some(&t));
            } else {
                self.load_thumb(c.id, &pic);
            }
            root.append(&pic);
        } else {
            root.append(
                &gtk::Label::builder()
                    .label(c.preview.as_str())
                    .xalign(0.0)
                    .yalign(0.0)
                    .wrap(true)
                    .wrap_mode(gtk::pango::WrapMode::WordChar)
                    .lines(if grid { 5 } else { 2 })
                    .ellipsize(gtk::pango::EllipsizeMode::End)
                    .css_classes(["clip-preview"])
                    .build(),
            );
        }

        let foot = gtk::Box::new(gtk::Orientation::Horizontal, 4);
        let tags = gtk::Box::new(gtk::Orientation::Horizontal, 4);
        tags.set_hexpand(true);
        for t in &c.tags {
            tags.append(
                &gtk::Label::builder()
                    .label(format!("#{t}"))
                    .css_classes(["tag-label"])
                    .build(),
            );
        }
        foot.append(&tags);

        let copy = icon_button("edit-copy-symbolic", "Copier");
        let w = Rc::downgrade(self);
        let id = c.id;
        copy.connect_clicked(move |b| {
            if let Some(p) = w.upgrade() {
                p.copy(id, b);
            }
        });
        let pin = gtk::ToggleButton::builder()
            .icon_name("view-pin-symbolic")
            .active(c.pinned)
            .tooltip_text(if c.pinned { "Désépingler" } else { "Épingler" })
            .css_classes(["flat", "circular"])
            .build();
        pin.update_property(&[gtk::accessible::Property::Label("Épingler")]);
        let w = Rc::downgrade(self);
        pin.connect_toggled(move |b| {
            if let Some(p) = w.upgrade() {
                p.set_pinned(id, b.is_active());
            }
        });
        let tag_btn = gtk::MenuButton::builder()
            .label("Tags")
            .tooltip_text("Modifier les tags")
            .css_classes(["flat"])
            .popover(&self.tag_popover(c))
            .build();
        let del = icon_button("user-trash-symbolic", "Supprimer");
        let w = Rc::downgrade(self);
        del.connect_clicked(move |_| {
            if let Some(p) = w.upgrade() {
                p.delete(vec![id]);
            }
        });
        for b in [
            copy.upcast_ref::<gtk::Widget>(),
            pin.upcast_ref(),
            tag_btn.upcast_ref(),
            del.upcast_ref(),
        ] {
            b.set_sensitive(!selecting);
            foot.append(b);
        }
        root.append(&foot);

        if selecting {
            let click = gtk::GestureClick::new();
            let w = Rc::downgrade(self);
            click.connect_released(move |_, _, _, _| {
                let Some(p) = w.upgrade() else { return };
                {
                    let mut s = p.state.borrow_mut();
                    if !s.selected.remove(&id) {
                        s.selected.insert(id);
                    }
                }
                p.render();
            });
            root.add_controller(click);
        }
        root.upcast()
    }

    fn tag_popover(self: &Rc<Self>, c: &ClipEntry) -> gtk::Popover {
        let entry = gtk::Entry::builder()
            .text(c.tags.join(", "))
            .placeholder_text("travail, rust, adresse…")
            .activates_default(true)
            .build();
        entry.update_property(&[gtk::accessible::Property::Label("Tags séparés par des virgules")]);
        let save = gtk::Button::builder()
            .label("Enregistrer")
            .css_classes(["suggested-action"])
            .build();
        let b = gtk::Box::new(gtk::Orientation::Vertical, 8);
        b.set_margin_top(8);
        b.set_margin_bottom(8);
        b.set_margin_start(8);
        b.set_margin_end(8);
        b.set_width_request(280);
        b.append(
            &gtk::Label::builder()
                .label("Tags séparés par des virgules")
                .xalign(0.0)
                .css_classes(["card-title"])
                .build(),
        );
        b.append(&entry);
        b.append(&hint(if c.pinned {
            "Vider les tags ne désépingle pas l'élément."
        } else {
            "Ajouter un tag épingle l'élément : il sera conservé après la déconnexion."
        }));
        b.append(&save);
        let pop = gtk::Popover::builder().child(&b).build();
        pop.set_default_widget(Some(&save));
        let w = Rc::downgrade(self);
        let id = c.id;
        let was_pinned = c.pinned;
        let p2 = pop.clone();
        save.connect_clicked(move |_| {
            let tags = parse_tag_input(&entry.text());
            p2.popdown();
            if let Some(p) = w.upgrade() {
                p.save_tags(id, tags, was_pinned);
            }
        });
        pop
    }

    fn load_thumb(self: &Rc<Self>, id: u64, pic: &gtk::Picture) {
        let w = Rc::downgrade(self);
        let ctx = self.ctx.clone();
        let pic = pic.clone();
        Ctx::spawn(async move {
            let Ok((_, data)) = ctx
                .app
                .backend
                .clips(move |c| async move { c.get(id).await })
                .await
            else {
                return;
            };
            let Ok(tex) = gdk::Texture::from_bytes(&glib::Bytes::from_owned(data)) else {
                pic.set_alternative_text(Some("Image illisible"));
                return;
            };
            pic.set_paintable(Some(&tex));
            if let Some(p) = w.upgrade() {
                p.state.borrow_mut().thumbs.insert(id, tex);
            }
        });
    }

    fn copy(&self, id: u64, widget: &gtk::Button) {
        let ctx = self.ctx.clone();
        let clipboard = widget.clipboard();
        Ctx::spawn(async move {
            match ctx
                .app
                .backend
                .clips(move |c| async move { c.get(id).await })
                .await
            {
                Ok((mime, data)) if mime.starts_with("image/") => {
                    match gdk::Texture::from_bytes(&glib::Bytes::from_owned(data)) {
                        Ok(t) => {
                            clipboard.set_texture(&t);
                            ctx.toast("Image copiée.");
                        }
                        Err(_) => ctx.toast("Image illisible."),
                    }
                }
                Ok((_, data)) => {
                    clipboard.set_text(&String::from_utf8_lossy(&data));
                    ctx.toast("Texte copié.");
                }
                Err(e) => ctx.error(&e),
            }
        });
    }

    fn set_pinned(&self, id: u64, pinned: bool) {
        let ctx = self.ctx.clone();
        Ctx::spawn(async move {
            match ctx
                .app
                .backend
                .clips(move |c| async move { c.set_pinned(id, pinned).await })
                .await
            {
                Ok(()) => ctx.toast(if pinned {
                    "Épinglé : conservé après la déconnexion."
                } else {
                    "Désépinglé : effacé à la fin de la session."
                }),
                Err(e) => ctx.error(&e),
            }
        });
    }

    fn save_tags(&self, id: u64, tags: Vec<String>, was_pinned: bool) {
        let ctx = self.ctx.clone();
        let tagging = !tags.is_empty();
        Ctx::spawn(async move {
            match ctx
                .app
                .backend
                .clips(move |c| async move { c.set_tags(id, &tags).await })
                .await
            {
                Ok(_) if tagging && !was_pinned => {
                    ctx.toast("Tags enregistrés ; l'élément est maintenant épinglé.");
                }
                Ok(_) => ctx.toast("Tags enregistrés."),
                Err(e) => ctx.error(&e),
            }
        });
    }

    fn delete(&self, ids: Vec<u64>) {
        let ctx = self.ctx.clone();
        Ctx::spawn(async move {
            match ctx
                .app
                .backend
                .clips(move |c| async move { c.delete(&ids).await })
                .await
            {
                Ok(n) => ctx.toast(&format!("{n} élément(s) supprimé(s).")),
                Err(e) => ctx.error(&e),
            }
        });
    }

    fn confirm_delete_selected(self: &Rc<Self>, anchor: &gtk::Button) {
        let ids: Vec<u64> = self.state.borrow().selected.iter().copied().collect();
        if ids.is_empty() {
            return;
        }
        let dialog = adw::AlertDialog::new(
            Some(&format!("Supprimer {} élément(s) ?", ids.len())),
            Some("Les éléments épinglés sélectionnés seront aussi supprimés. C'est définitif."),
        );
        dialog.add_response("cancel", "Annuler");
        dialog.add_response("delete", "Supprimer");
        dialog.set_response_appearance("delete", adw::ResponseAppearance::Destructive);
        dialog.set_close_response("cancel");
        let w = Rc::downgrade(self);
        let anchor = anchor.clone();
        Ctx::spawn(async move {
            if dialog.choose_future(Some(&anchor)).await != "delete" {
                return;
            }
            if let Some(p) = w.upgrade() {
                p.delete(ids);
                p.select_toggle.set_active(false);
            }
        });
    }

    fn confirm_clear(&self, anchor: &gtk::Button) {
        let dialog = adw::AlertDialog::new(
            Some("Effacer l'historique ?"),
            Some("Tous les éléments non épinglés sont supprimés. Les éléments épinglés (et tagués) restent."),
        );
        dialog.add_response("cancel", "Annuler");
        dialog.add_response("clear", "Effacer");
        dialog.set_response_appearance("clear", adw::ResponseAppearance::Destructive);
        dialog.set_close_response("cancel");
        let ctx = self.ctx.clone();
        let anchor = anchor.clone();
        Ctx::spawn(async move {
            if dialog.choose_future(Some(&anchor)).await != "clear" {
                return;
            }
            match ctx
                .app
                .backend
                .clips(|c| async move { c.clear_unpinned().await })
                .await
            {
                Ok(n) => ctx.toast(&format!("{n} élément(s) effacé(s).")),
                Err(e) => ctx.error(&e),
            }
        });
    }
}
