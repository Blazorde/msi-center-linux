//! « Mes courbes » (ADR-0016) : la bibliothèque de courbes du propriétaire, autant qu'il en veut. Enregistrer
//! photographie les courbes en vigueur dans le profil actuel ; Appliquer les recharge en Personnalisé.
//!
//! Le service reste l'autorité : il valide le nom et chaque courbe (S1–S3) à l'enregistrement comme à
//! l'application.

use std::cell::RefCell;
use std::rc::Rc;

use adw::prelude::*;
use msi_core::library::MAX_NAME_CHARS;

use super::widgets::{Ctx, glass, hint, section_title};

/// The library card.
pub struct CurveLibrary {
    /// Section (title and card).
    pub root: gtk::Box,
    ctx: Rc<Ctx>,
    name: adw::EntryRow,
    save: gtk::Button,
    list: gtk::ListBox,
    empty: gtk::Label,
    names: RefCell<Vec<String>>,
    read_only: RefCell<bool>,
    changed: RefCell<Option<Rc<dyn Fn()>>>,
}

impl std::fmt::Debug for CurveLibrary {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CurveLibrary").finish_non_exhaustive()
    }
}

impl CurveLibrary {
    /// Builds the card (empty until [`Self::load`]).
    pub fn new(ctx: &Rc<Ctx>) -> Rc<Self> {
        let name = adw::EntryRow::builder()
            .title("Nom de la courbe")
            .show_apply_button(false)
            .build();
        name.set_max_length(i32::try_from(MAX_NAME_CHARS).unwrap_or(i32::MAX));
        let save = gtk::Button::builder()
            .label("Enregistrer")
            .tooltip_text("Enregistrer les courbes en vigueur dans ce profil sous ce nom")
            .valign(gtk::Align::Center)
            .css_classes(["suggested-action"])
            .sensitive(false)
            .build();
        name.add_suffix(&save);
        let form = gtk::ListBox::builder()
            .selection_mode(gtk::SelectionMode::None)
            .css_classes(["boxed-list"])
            .build();
        form.append(&name);

        let list = gtk::ListBox::builder()
            .selection_mode(gtk::SelectionMode::None)
            .css_classes(["boxed-list"])
            .build();
        let empty = hint("Aucune courbe enregistrée pour l'instant.");

        let inner = gtk::Box::new(gtk::Orientation::Vertical, 10);
        inner.append(&hint(
            "Enregistrez les courbes en vigueur (CPU, paliers compris, et GPU) sous un nom, autant que vous \
             voulez, puis réappliquez-les dans n'importe quel profil : elles passent en Personnalisé.",
        ));
        inner.append(&form);
        inner.append(&empty);
        inner.append(&list);
        let root = gtk::Box::new(gtk::Orientation::Vertical, 10);
        root.append(&section_title("MES COURBES"));
        root.append(&glass(&inner));

        let lib = Rc::new(Self {
            root,
            ctx: ctx.clone(),
            name,
            save,
            list,
            empty,
            names: RefCell::default(),
            read_only: RefCell::new(false),
            changed: RefCell::new(None),
        });
        lib.connect();
        lib
    }

    /// `f` runs after curves were applied, so the page reloads what is in force.
    pub fn connect_changed(&self, f: impl Fn() + 'static) {
        *self.changed.borrow_mut() = Some(Rc::new(f));
    }

    /// Read-only mode (unsupported EC firmware or S9 stop): nothing can be applied.
    pub fn set_read_only(self: &Rc<Self>, read_only: bool) {
        *self.read_only.borrow_mut() = read_only;
        self.rebuild();
    }

    fn connect(self: &Rc<Self>) {
        let w = Rc::downgrade(self);
        self.name.connect_changed(move |e| {
            if let Some(p) = w.upgrade() {
                p.save.set_sensitive(!e.text().trim().is_empty());
            }
        });
        let w = Rc::downgrade(self);
        self.name.connect_entry_activated(move |_| {
            if let Some(p) = w.upgrade() {
                p.save_current();
            }
        });
        let w = Rc::downgrade(self);
        self.save.connect_clicked(move |_| {
            if let Some(p) = w.upgrade() {
                p.save_current();
            }
        });
    }

    /// Asks the service for the library names.
    pub fn load(self: &Rc<Self>) {
        let w = Rc::downgrade(self);
        let ctx = self.ctx.clone();
        Ctx::spawn(async move {
            match ctx
                .app
                .backend
                .daemon(|d| async move { d.saved_curves().await })
                .await
            {
                Ok(names) => {
                    if let Some(p) = w.upgrade() {
                        p.set_names(names);
                    }
                }
                // Older daemon without the library: the card stays empty.
                Err(e) if !e.is_unavailable() => tracing::debug!("saved curves: {e}"),
                Err(_) => {}
            }
        });
    }

    fn set_names(self: &Rc<Self>, names: Vec<String>) {
        *self.names.borrow_mut() = names;
        self.rebuild();
    }

    fn rebuild(self: &Rc<Self>) {
        while let Some(c) = self.list.first_child() {
            self.list.remove(&c);
        }
        let names = self.names.borrow().clone();
        let read_only = *self.read_only.borrow();
        self.empty.set_visible(names.is_empty());
        self.list.set_visible(!names.is_empty());
        for name in names {
            let row = adw::ActionRow::builder()
                .title(gtk::glib::markup_escape_text(&name))
                .build();
            let apply = gtk::Button::builder()
                .label("Appliquer")
                .valign(gtk::Align::Center)
                .sensitive(!read_only)
                .build();
            apply.update_property(&[gtk::accessible::Property::Label(&format!("Appliquer « {name} »"))]);
            let delete = gtk::Button::builder()
                .icon_name("user-trash-symbolic")
                .tooltip_text("Supprimer cette courbe")
                .valign(gtk::Align::Center)
                .css_classes(["flat"])
                .build();
            delete.update_property(&[gtk::accessible::Property::Label(&format!("Supprimer « {name} »"))]);
            row.add_suffix(&apply);
            row.add_suffix(&delete);
            let w = Rc::downgrade(self);
            let n = name.clone();
            apply.connect_clicked(move |_| {
                if let Some(p) = w.upgrade() {
                    p.apply(n.clone());
                }
            });
            let w = Rc::downgrade(self);
            delete.connect_clicked(move |_| {
                if let Some(p) = w.upgrade() {
                    p.delete(name.clone());
                }
            });
            self.list.append(&row);
        }
    }

    fn save_current(self: &Rc<Self>) {
        let name = self.name.text().trim().to_owned();
        if name.is_empty() {
            return;
        }
        let w = Rc::downgrade(self);
        let ctx = self.ctx.clone();
        Ctx::spawn(async move {
            let n = name.clone();
            match ctx
                .app
                .backend
                .daemon(move |d| async move { d.save_curves(&n).await })
                .await
            {
                Ok(names) => {
                    ctx.toast(&format!("Courbes enregistrées sous « {name} »."));
                    if let Some(p) = w.upgrade() {
                        p.name.set_text("");
                        p.set_names(names);
                    }
                }
                Err(e) => ctx.error(&e),
            }
        });
    }

    fn apply(self: &Rc<Self>, name: String) {
        let w = Rc::downgrade(self);
        let ctx = self.ctx.clone();
        Ctx::spawn(async move {
            let n = name.clone();
            match ctx
                .app
                .backend
                .daemon(move |d| async move { d.apply_saved_curves(&n).await })
                .await
            {
                Ok(()) => ctx.toast(&format!("« {name} » appliquée (Personnalisé).")),
                Err(e) => ctx.error(&e),
            }
            if let Some(p) = w.upgrade() {
                let changed = p.changed.borrow().clone();
                if let Some(f) = changed {
                    f();
                }
            }
        });
    }

    fn delete(self: &Rc<Self>, name: String) {
        let w = Rc::downgrade(self);
        let ctx = self.ctx.clone();
        Ctx::spawn(async move {
            let n = name.clone();
            match ctx
                .app
                .backend
                .daemon(move |d| async move { d.delete_saved_curves(&n).await })
                .await
            {
                Ok(names) => {
                    ctx.toast(&format!("« {name} » supprimée."));
                    if let Some(p) = w.upgrade() {
                        p.set_names(names);
                    }
                }
                Err(e) => ctx.error(&e),
            }
        });
    }
}
