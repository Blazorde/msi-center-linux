//! Éditeur de la courbe à paliers du CPU (ADR-0013): autant de paliers que souhaité, chacun avec sa
//! température de montée, sa puissance et sa température de retombée.
//!
//! La logique (brouillon, validation, messages) est dans `msi_center::stages`, testée sans affichage ; ce
//! module n'est que le widget. Le service reste l'autorité : il valide à nouveau (S1–S3) et le plancher de
//! sécurité S2 est affiché, jamais appliqué en silence.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use adw::prelude::*;
use msi_center::format::NNBSP;
use msi_center::stages::{self, Draft, StagedState};
use msi_core::staged::MAX_STAGES;

use super::fanchart::{FanChart, StagedData};
use super::widgets::{Ctx, glass, hint, section_title};

/// Range accepted for the rise and fall-back temperatures (°C), as the service.
const TEMP_MIN: f64 = 20.0;
/// Highest temperature (°C).
const TEMP_MAX: f64 = 100.0;

#[derive(Debug)]
struct EditState {
    /// What the service reported; `None` before the first answer or on a daemon without ADR-0013.
    base: Option<StagedState>,
    draft: Draft,
    /// The CPU fan is the one shown.
    cpu: bool,
    read_only: bool,
}

impl EditState {
    fn dirty(&self) -> bool {
        self.base
            .as_ref()
            .is_some_and(|b| Draft::from_curve(&b.curve) != self.draft)
    }

    fn supported(&self) -> bool {
        self.base.as_ref().is_some_and(|b| b.supported)
    }
}

/// The editor card.
pub struct StagesEditor {
    /// Section (title and card), hidden unless the CPU fan is shown and the service offers staged curves.
    pub root: gtk::Box,
    ctx: Rc<Ctx>,
    chart: FanChart,
    state: RefCell<EditState>,
    note: gtk::Label,
    summary: gtk::Label,
    idle: gtk::SpinButton,
    list: gtk::ListBox,
    add: gtk::Button,
    apply: gtk::Button,
    revert: gtk::Button,
    reset: gtk::Button,
    error: gtk::Revealer,
    error_label: gtk::Label,
    floors: gtk::Revealer,
    floors_list: gtk::Box,
    syncing: Cell<bool>,
    changed: RefCell<Option<Rc<dyn Fn()>>>,
}

impl std::fmt::Debug for StagesEditor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StagesEditor").finish_non_exhaustive()
    }
}

fn spin(min: f64, max: f64, value: u8, label: &str) -> gtk::SpinButton {
    let s = gtk::SpinButton::with_range(min, max, 1.0);
    s.set_value(f64::from(value));
    s.set_width_chars(4);
    s.set_numeric(true);
    s.update_property(&[gtk::accessible::Property::Label(label)]);
    s
}

fn captioned(caption: &str, spin: &gtk::SpinButton) -> gtk::Box {
    let b = gtk::Box::new(gtk::Orientation::Vertical, 2);
    b.append(
        &gtk::Label::builder()
            .label(caption)
            .xalign(0.0)
            .css_classes(["dim", "caption"])
            .build(),
    );
    b.append(spin);
    b
}

fn value_of(s: &gtk::SpinButton) -> u8 {
    u8::try_from(s.value_as_int()).unwrap_or(u8::MAX)
}

impl StagesEditor {
    /// Builds the editor. `chart` is the page's chart: the editor draws the staged curve on it.
    #[allow(clippy::too_many_lines, reason = "widget tree")]
    pub fn new(ctx: &Rc<Ctx>, chart: FanChart) -> Rc<Self> {
        let intro = hint(&format!(
            "Autant de paliers que vous voulez (jusqu'à {MAX_STAGES}). Chaque palier a sa température de \
             montée, sa puissance et sa température de retombée : le ventilateur y reste tant que la \
             température ne repasse pas sous la retombée. Réservé au ventilateur CPU, que le service \
             pilote en direct ; le GPU garde ses 7 paliers."
        ));
        let note = gtk::Label::builder()
            .xalign(0.0)
            .wrap(true)
            .css_classes(["caption"])
            .build();
        let summary = gtk::Label::builder()
            .xalign(0.0)
            .css_classes(["dim", "numeric"])
            .build();

        let idle = spin(0.0, 100.0, 0, "Puissance au repos");
        let idle_row = gtk::Box::new(gtk::Orientation::Horizontal, 12);
        idle_row.append(
            &gtk::Label::builder()
                .label("Repos")
                .xalign(0.0)
                .width_chars(8)
                .css_classes(["heading"])
                .build(),
        );
        idle_row.append(&captioned(
            &format!("Puissance sous le premier palier ({NNBSP}%)"),
            &idle,
        ));

        let list = gtk::ListBox::builder()
            .selection_mode(gtk::SelectionMode::None)
            .css_classes(["boxed-list"])
            .build();
        let list_scroll = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .min_content_height(120)
            .max_content_height(420)
            .propagate_natural_height(true)
            .child(&list)
            .build();

        let add = gtk::Button::builder()
            .label("Ajouter un palier")
            .icon_name("list-add-symbolic")
            .css_classes(["pill"])
            .build();
        let apply = gtk::Button::builder()
            .label("Appliquer la courbe à paliers")
            .css_classes(["pill", "suggested-action"])
            .sensitive(false)
            .build();
        let revert = gtk::Button::builder()
            .label("Annuler les modifications")
            .css_classes(["pill"])
            .sensitive(false)
            .build();
        let reset = gtk::Button::builder()
            .label("Supprimer la courbe à paliers")
            .tooltip_text("Revient à la courbe personnalisée exponentielle")
            .css_classes(["pill"])
            .sensitive(false)
            .build();
        let actions = gtk::FlowBox::builder()
            .selection_mode(gtk::SelectionMode::None)
            .max_children_per_line(4)
            .column_spacing(8)
            .row_spacing(8)
            .build();
        for b in [&add, &apply, &revert, &reset] {
            actions.append(b);
        }

        let error_label = gtk::Label::builder()
            .xalign(0.0)
            .wrap(true)
            .css_classes(["raised-mark"])
            .build();
        let error_box = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        error_box.append(&gtk::Image::from_icon_name("dialog-error-symbolic"));
        error_box.append(&error_label);
        let error = gtk::Revealer::builder().child(&error_box).build();

        let floors_list = gtk::Box::new(gtk::Orientation::Vertical, 6);
        let floors_box = gtk::Box::new(gtk::Orientation::Vertical, 8);
        let floors_head = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        floors_head.append(&gtk::Image::from_icon_name("dialog-warning-symbolic"));
        floors_head.append(
            &gtk::Label::builder()
                .label("Planchers de sécurité (S2) qui prennent le relais")
                .xalign(0.0)
                .wrap(true)
                .css_classes(["card-title"])
                .build(),
        );
        floors_box.append(&floors_head);
        floors_box.append(&hint(
            "Vos paliers sont plus lents que les minimums de sécurité (30 % dès 60 °C, 50 % dès 75 °C, \
             100 % dès 85 °C). Le service applique donc le plus grand des deux ; rien n'est caché.",
        ));
        floors_box.append(&floors_list);
        let floors_card = glass(&floors_box);
        floors_card.add_css_class("warning-card");
        let floors = gtk::Revealer::builder()
            .transition_type(gtk::RevealerTransitionType::SlideDown)
            .child(&floors_card)
            .build();

        let inner = gtk::Box::new(gtk::Orientation::Vertical, 10);
        inner.append(&intro);
        inner.append(&note);
        inner.append(&idle_row);
        inner.append(&list_scroll);
        inner.append(&summary);
        inner.append(&error);
        inner.append(&floors);
        inner.append(&actions);
        let root = gtk::Box::new(gtk::Orientation::Vertical, 10);
        root.append(&section_title("COURBE À PALIERS (CPU)"));
        root.append(&glass(&inner));
        root.set_visible(false);

        let editor = Rc::new(Self {
            root,
            ctx: ctx.clone(),
            chart,
            state: RefCell::new(EditState {
                base: None,
                draft: Draft {
                    idle: 0,
                    stages: Vec::new(),
                },
                cpu: false,
                read_only: false,
            }),
            note,
            summary,
            idle,
            list,
            add,
            apply,
            revert,
            reset,
            error,
            error_label,
            floors,
            floors_list,
            syncing: Cell::new(false),
            changed: RefCell::new(None),
        });
        editor.connect();
        editor
    }

    /// `f` runs after the service accepted a change, so the page reloads what is in force.
    pub fn connect_changed(&self, f: impl Fn() + 'static) {
        *self.changed.borrow_mut() = Some(Rc::new(f));
    }

    fn connect(self: &Rc<Self>) {
        let w = Rc::downgrade(self);
        self.idle.connect_value_changed(move |s| {
            if let Some(p) = w.upgrade()
                && !p.syncing.get()
            {
                p.state.borrow_mut().draft.set_idle(value_of(s));
                p.after_edit();
            }
        });
        let w = Rc::downgrade(self);
        self.add.connect_clicked(move |_| {
            if let Some(p) = w.upgrade() {
                p.state.borrow_mut().draft.add();
                p.rebuild_rows();
                p.after_edit();
            }
        });
        let w = Rc::downgrade(self);
        self.apply.connect_clicked(move |_| {
            if let Some(p) = w.upgrade() {
                p.apply();
            }
        });
        let w = Rc::downgrade(self);
        self.revert.connect_clicked(move |_| {
            if let Some(p) = w.upgrade() {
                p.revert();
            }
        });
        let w = Rc::downgrade(self);
        self.reset.connect_clicked(move |_| {
            if let Some(p) = w.upgrade() {
                p.reset();
            }
        });
    }

    /// The page loaded the curve of a fan. `staged` is the service's answer for the CPU (`None` on a
    /// service without staged curves, or for the GPU); `cpu` tells which fan is shown.
    ///
    /// An unapplied draft survives a reload (the page reloads on every live-target change); it is replaced
    /// only when the service's curve itself changed.
    pub fn set_state(self: &Rc<Self>, staged: Option<StagedState>, cpu: bool) {
        {
            let mut st = self.state.borrow_mut();
            let keep_draft =
                st.dirty() && st.base.as_ref().map(|b| &b.curve) == staged.as_ref().map(|b| &b.curve);
            if let Some(new) = &staged
                && !keep_draft
            {
                st.draft = Draft::from_curve(&new.curve);
            }
            st.base = staged;
            st.cpu = cpu;
        }
        self.root.set_visible(cpu && self.state.borrow().base.is_some());
        self.rebuild_rows();
        self.after_edit();
    }

    /// Read-only mode (unsupported EC firmware or S9 stop): nothing can be applied.
    pub fn set_read_only(&self, read_only: bool) {
        self.state.borrow_mut().read_only = read_only;
        self.after_edit();
    }

    /// One row per stage. Rebuilt only when stages are added or removed, so typing in a field keeps focus.
    fn rebuild_rows(self: &Rc<Self>) {
        while let Some(c) = self.list.first_child() {
            self.list.remove(&c);
        }
        self.syncing.set(true);
        self.idle.set_value(f64::from(self.state.borrow().draft.idle));
        let stages_now = self.state.borrow().draft.stages.clone();
        let count = stages_now.len();
        for (i, stage) in stages_now.into_iter().enumerate() {
            let n = i + 1;
            let up = spin(
                TEMP_MIN,
                TEMP_MAX,
                stage.up,
                &format!("Palier {n}, température de montée"),
            );
            let speed = spin(0.0, 100.0, stage.speed, &format!("Palier {n}, puissance"));
            let down = spin(
                TEMP_MIN,
                TEMP_MAX,
                stage.down,
                &format!("Palier {n}, température de retombée"),
            );

            let w = Rc::downgrade(self);
            up.connect_value_changed(move |s| {
                if let Some(p) = w.upgrade()
                    && !p.syncing.get()
                {
                    p.state.borrow_mut().draft.set_up(i, value_of(s));
                    p.after_edit();
                }
            });
            let w = Rc::downgrade(self);
            speed.connect_value_changed(move |s| {
                if let Some(p) = w.upgrade()
                    && !p.syncing.get()
                {
                    p.state.borrow_mut().draft.set_speed(i, value_of(s));
                    p.after_edit();
                }
            });
            let w = Rc::downgrade(self);
            down.connect_value_changed(move |s| {
                if let Some(p) = w.upgrade()
                    && !p.syncing.get()
                {
                    p.state.borrow_mut().draft.set_down(i, value_of(s));
                    p.after_edit();
                }
            });

            let row = gtk::Box::new(gtk::Orientation::Horizontal, 12);
            row.set_margin_top(8);
            row.set_margin_bottom(8);
            row.set_margin_start(12);
            row.set_margin_end(12);
            row.append(
                &gtk::Label::builder()
                    .label(format!("Palier {n}"))
                    .xalign(0.0)
                    .width_chars(8)
                    .css_classes(["heading"])
                    .build(),
            );
            row.append(&captioned(&format!("Monte à ≥ ({NNBSP}°C)"), &up));
            row.append(&captioned(&format!("Puissance ({NNBSP}%)"), &speed));
            row.append(&captioned(&format!("Retombe sous ({NNBSP}°C)"), &down));
            let spacer = gtk::Box::new(gtk::Orientation::Horizontal, 0);
            spacer.set_hexpand(true);
            row.append(&spacer);

            if i + 1 < count {
                let insert = gtk::Button::builder()
                    .icon_name("list-add-symbolic")
                    .tooltip_text("Insérer un palier après celui-ci")
                    .valign(gtk::Align::End)
                    .css_classes(["flat"])
                    .build();
                insert.update_property(&[gtk::accessible::Property::Label(&format!(
                    "Insérer un palier après le palier {n}"
                ))]);
                let w = Rc::downgrade(self);
                insert.connect_clicked(move |_| {
                    if let Some(p) = w.upgrade() {
                        if p.state.borrow_mut().draft.insert_after(i).is_none() {
                            p.ctx.toast("Pas de place entre ces deux paliers.");
                            return;
                        }
                        p.rebuild_rows();
                        p.after_edit();
                    }
                });
                row.append(&insert);
            }
            let remove = gtk::Button::builder()
                .icon_name("user-trash-symbolic")
                .tooltip_text("Supprimer ce palier")
                .valign(gtk::Align::End)
                .css_classes(["flat"])
                .build();
            remove.update_property(&[gtk::accessible::Property::Label(&format!(
                "Supprimer le palier {n}"
            ))]);
            let w = Rc::downgrade(self);
            remove.connect_clicked(move |_| {
                if let Some(p) = w.upgrade() {
                    p.state.borrow_mut().draft.remove(i);
                    p.rebuild_rows();
                    p.after_edit();
                }
            });
            row.append(&remove);
            self.list.append(&row);
        }
        self.syncing.set(false);
    }

    /// Everything that depends on the draft but not on the number of rows: validation, floors, buttons,
    /// chart.
    fn after_edit(&self) {
        let st = self.state.borrow();
        let valid = st.draft.validate();
        let dirty = st.dirty();
        let supported = st.supported();
        let editable = supported && !st.read_only;
        let (saved, active) = st.base.as_ref().map_or((false, false), |b| (b.saved, b.active));

        self.note.set_label(&if !supported {
            "Ce firmware n'autorise pas le pilotage en direct du ventilateur CPU : les courbes à paliers \
             sont refusées."
                .to_owned()
        } else if st.read_only {
            "Mode lecture seule : rien ne peut être appliqué.".to_owned()
        } else if active {
            "Courbe à paliers active : le service pilote le ventilateur CPU en direct, palier par palier."
                .to_owned()
        } else if saved {
            "Courbe à paliers enregistrée mais pas en vigueur (le profil de ventilation n'est pas \
             Personnalisé). « Appliquer » la remet en vigueur."
                .to_owned()
        } else {
            "Pas encore de courbe à paliers : le point de départ reprend votre courbe personnalisée. \
             Modifiez-le puis « Appliquer »."
                .to_owned()
        });

        match &valid {
            Ok(curve) => {
                self.error.set_reveal_child(false);
                self.summary.set_label(&stages::summary(curve));
                while let Some(c) = self.floors_list.first_child() {
                    self.floors_list.remove(&c);
                }
                let raised = curve.raised_floors();
                for r in &raised {
                    self.floors_list.append(
                        &gtk::Label::builder()
                            .label(format!("▲ {}", stages::floor_fr(r)))
                            .xalign(0.0)
                            .wrap(true)
                            .build(),
                    );
                }
                self.floors.set_reveal_child(!raised.is_empty());
            }
            Err(msg) => {
                self.error_label.set_label(msg);
                self.error.set_reveal_child(true);
                self.floors.set_reveal_child(false);
                self.summary.set_label("");
            }
        }

        self.add.set_sensitive(editable && st.draft.can_add());
        self.apply
            .set_sensitive(editable && valid.is_ok() && (dirty || !active));
        self.revert.set_sensitive(dirty);
        self.reset.set_sensitive(editable && saved);

        // Chart: the draft when it is valid and differs from what is in force (dashed), else what is in force.
        let applied = st.base.as_ref().filter(|b| b.active).map(|b| b.curve.clone());
        let shown = match valid {
            Ok(curve) if dirty => Some(StagedData {
                curve,
                applied,
                preview: true,
            }),
            _ => applied.map(|curve| StagedData {
                curve,
                applied: None,
                preview: false,
            }),
        };
        let cpu = st.cpu;
        drop(st);
        self.chart.update(|d| d.staged = if cpu { shown } else { None });
    }

    fn revert(self: &Rc<Self>) {
        {
            let mut st = self.state.borrow_mut();
            if let Some(b) = &st.base {
                st.draft = Draft::from_curve(&b.curve);
            }
        }
        self.rebuild_rows();
        self.after_edit();
    }

    fn notify_changed(&self) {
        let f = self.changed.borrow().clone();
        if let Some(f) = f {
            f();
        }
    }

    fn apply(self: &Rc<Self>) {
        let Ok(curve) = self.state.borrow().draft.validate() else {
            return;
        };
        let (idle, wire) = (curve.idle(), stages::stages_to_wire(&curve));
        let (count, floors) = (curve.stages().len(), curve.raised_floors().len());
        self.apply.set_sensitive(false);
        let w = Rc::downgrade(self);
        let ctx = self.ctx.clone();
        Ctx::spawn(async move {
            let res = ctx
                .app
                .backend
                .daemon(move |d| async move { d.set_cpu_stages(idle, &wire).await })
                .await;
            match res {
                Ok(_) if floors == 0 => ctx.toast(&format!("Courbe à paliers appliquée ({count}).")),
                Ok(_) => ctx.toast(&format!(
                    "Courbe à paliers appliquée ({count}) ; {floors} plancher(s) de sécurité S2 prennent le relais."
                )),
                Err(e) => ctx.error(&e),
            }
            if let Some(p) = w.upgrade() {
                p.notify_changed();
            }
        });
    }

    fn reset(self: &Rc<Self>) {
        let w = Rc::downgrade(self);
        let ctx = self.ctx.clone();
        Ctx::spawn(async move {
            match ctx
                .app
                .backend
                .daemon(|d| async move { d.reset_cpu_stages().await })
                .await
            {
                Ok(()) => ctx.toast("Courbe à paliers supprimée."),
                Err(e) => ctx.error(&e),
            }
            if let Some(p) = w.upgrade() {
                p.notify_changed();
            }
        });
    }
}
