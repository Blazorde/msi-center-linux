//! Ventilateurs: step chart with live marker, table, S2 safety report, curve editor with live preview.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;

use adw::prelude::*;
use msi_center::chart;
use msi_center::curve::{CurveSpec, Generated, Param, live_cpu_note};
use msi_center::dbus::FanCurveState;
use msi_center::errors::Service;
use msi_center::format::{self, NNBSP};
use msi_center::model::{self, DaemonStatus, Fan, Telemetry};

use super::fanchart::FanChart;
use super::widgets::{Ctx, Debounce, Gate, glass, hint, page_scroller, section_title};

#[derive(Debug, Default)]
struct State {
    fan: Option<Fan>,
    loaded: Option<FanCurveState>,
    edit: Option<CurveSpec>,
    preview: Option<Generated>,
    error: Option<String>,
    seq: u64,
    temps: Telemetry,
    status: Option<DaemonStatus>,
}

impl State {
    fn fan(&self) -> Fan {
        self.fan.unwrap_or(Fan::Cpu)
    }

    fn dirty(&self) -> bool {
        match (&self.edit, &self.loaded) {
            (Some(e), Some(l)) => *e != l.spec,
            _ => false,
        }
    }

    fn shown(&self) -> Option<&Generated> {
        if self.dirty() {
            self.preview.as_ref()
        } else {
            self.loaded.as_ref().map(|l| &l.generated)
        }
    }
}

struct ParamRow {
    scale: gtk::Scale,
    value: gtk::Label,
}

/// The page.
pub struct Fans {
    /// Root widget.
    pub gate: Gate,
    ctx: Rc<Ctx>,
    state: RefCell<State>,
    fan_switch: adw::ToggleGroup,
    origin: gtk::Label,
    chart: FanChart,
    cells: Vec<[gtk::Label; 7]>,
    report: gtk::Revealer,
    report_list: gtk::Box,
    error: gtk::Revealer,
    error_label: gtk::Label,
    params: HashMap<Param, ParamRow>,
    thresholds: gtk::Label,
    apply: gtk::Button,
    revert: gtk::Button,
    reset: gtk::Button,
    restore: gtk::Button,
    boost: adw::SwitchRow,
    syncing: Cell<bool>,
    debounce: Rc<Debounce>,
}

impl std::fmt::Debug for Fans {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Fans").finish_non_exhaustive()
    }
}

const ROWS: [&str; 5] = ["Palier", "Vitesse", "Monte à ≥", "Redescend <", "Sécurité S2"];

fn legend() -> gtk::Box {
    let b = gtk::Box::new(gtk::Orientation::Horizontal, 14);
    b.add_css_class("caption");
    for (sym, text) in [
        ("━", "paliers"),
        ("╍", "descente (hystérésis)"),
        ("┈", "courbe exponentielle"),
        ("▨", "plancher de sécurité S2"),
        ("▲", "palier relevé"),
        ("●", "température actuelle"),
    ] {
        let l = gtk::Label::builder()
            .label(format!("{sym} {text}"))
            .css_classes(["dim"])
            .build();
        b.append(&l);
    }
    b
}

impl Fans {
    /// Builds the page.
    #[allow(clippy::too_many_lines, reason = "widget tree")]
    pub fn new(ctx: &Rc<Ctx>) -> Rc<Self> {
        let content = gtk::Box::new(gtk::Orientation::Vertical, 14);

        let fan_switch = adw::ToggleGroup::builder()
            .css_classes(["profile-switch"])
            .homogeneous(true)
            .halign(gtk::Align::Start)
            .build();
        for fan in Fan::ALL {
            fan_switch.add(
                adw::Toggle::builder()
                    .name(fan.as_str())
                    .label(fan.label())
                    .build(),
            );
        }
        fan_switch.set_active_name(Some("cpu"));
        let origin = gtk::Label::builder()
            .xalign(0.0)
            .wrap(true)
            .css_classes(["dim"])
            .build();
        let head = gtk::Box::new(gtk::Orientation::Vertical, 8);
        head.append(&fan_switch);
        head.append(&origin);
        content.append(&head);

        // Chart.
        let chart = FanChart::new();
        let chart_box = gtk::Box::new(gtk::Orientation::Vertical, 8);
        chart_box.append(&chart.area);
        let legend_scroll = gtk::ScrolledWindow::builder()
            .vscrollbar_policy(gtk::PolicyType::Never)
            .child(&legend())
            .build();
        chart_box.append(&legend_scroll);
        content.append(&glass(&chart_box));

        // Table.
        let grid = gtk::Grid::builder()
            .column_spacing(4)
            .row_spacing(4)
            .column_homogeneous(true)
            .build();
        let mut cells = Vec::new();
        for (r, title) in (0i32..).zip(ROWS) {
            grid.attach(
                &gtk::Label::builder()
                    .label(title)
                    .xalign(0.0)
                    .css_classes(["dim", "caption-heading"])
                    .build(),
                0,
                r,
                1,
                1,
            );
            let row: [gtk::Label; 7] = std::array::from_fn(|_| {
                gtk::Label::builder()
                    .css_classes(["step-cell", "numeric"])
                    .build()
            });
            for (c, l) in (1i32..).zip(row.iter()) {
                grid.attach(l, c, r, 1, 1);
            }
            cells.push(row);
        }
        let table_scroll = gtk::ScrolledWindow::builder()
            .vscrollbar_policy(gtk::PolicyType::Never)
            .propagate_natural_height(true)
            .child(&grid)
            .build();
        let table_box = gtk::Box::new(gtk::Orientation::Vertical, 8);
        table_box.append(
            &gtk::Label::builder()
                .label("Table écrite dans l'EC")
                .xalign(0.0)
                .css_classes(["card-title"])
                .build(),
        );
        table_box.append(&table_scroll);
        content.append(&glass(&table_box));

        // S2 report.
        let report_list = gtk::Box::new(gtk::Orientation::Vertical, 6);
        let report_box = gtk::Box::new(gtk::Orientation::Vertical, 8);
        let report_head = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        report_head.append(&gtk::Image::from_icon_name("dialog-warning-symbolic"));
        report_head.append(
            &gtk::Label::builder()
                .label("Paliers relevés par les planchers de sécurité (S2)")
                .xalign(0.0)
                .wrap(true)
                .css_classes(["card-title"])
                .build(),
        );
        report_box.append(&report_head);
        report_box.append(&hint(
            "Votre courbe est plus lente que les minimums de sécurité (30 % dès 60 °C, 50 % dès 75 °C, \
             100 % dès 85 °C). Ces paliers sont donc plus rapides que demandé ; rien n'est caché.",
        ));
        report_box.append(&report_list);
        let report_card = glass(&report_box);
        report_card.add_css_class("warning-card");
        let report = gtk::Revealer::builder()
            .transition_type(gtk::RevealerTransitionType::SlideDown)
            .child(&report_card)
            .build();
        content.append(&report);

        // Editor.
        content.append(&section_title("RÉGLAGE DE LA COURBE"));
        let error_label = gtk::Label::builder()
            .xalign(0.0)
            .wrap(true)
            .css_classes(["raised-mark"])
            .build();
        let error_box = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        error_box.append(&gtk::Image::from_icon_name("dialog-error-symbolic"));
        error_box.append(&error_label);
        let error = gtk::Revealer::builder().child(&error_box).build();
        content.append(&error);

        let group = adw::PreferencesGroup::builder()
            .title("Paramètres")
            .description(
                "L'aperçu est calculé par le service sans rien écrire. « Appliquer » remplace la courbe \
                 du profil actif pour ce ventilateur.",
            )
            .build();
        let mut params = HashMap::new();
        for p in Param::ALL {
            let r = p.range();
            let scale = gtk::Scale::with_range(gtk::Orientation::Horizontal, r.min, r.max, r.step);
            scale.set_digits(i32::from(p == Param::K));
            scale.set_draw_value(false);
            scale.set_width_request(200);
            scale.set_hexpand(true);
            scale.set_valign(gtk::Align::Center);
            scale.update_property(&[gtk::accessible::Property::Label(p.title())]);
            let value = gtk::Label::builder()
                .width_chars(7)
                .xalign(1.0)
                .css_classes(["numeric"])
                .build();
            let row = adw::ActionRow::builder()
                .title(p.title())
                .subtitle(p.subtitle())
                .build();
            let suffix = gtk::Box::new(gtk::Orientation::Horizontal, 8);
            suffix.append(&scale);
            suffix.append(&value);
            row.add_suffix(&suffix);
            row.set_activatable_widget(Some(&scale));
            group.add(&row);
            params.insert(p, ParamRow { scale, value });
        }
        let thresholds = gtk::Label::builder()
            .xalign(0.0)
            .wrap(true)
            .css_classes(["dim", "numeric"])
            .build();
        let editor = gtk::Box::new(gtk::Orientation::Vertical, 10);
        editor.append(&group);
        editor.append(&thresholds);

        let apply = gtk::Button::builder()
            .label("Appliquer")
            .css_classes(["pill", "suggested-action"])
            .sensitive(false)
            .build();
        let revert = gtk::Button::builder()
            .label("Annuler les modifications")
            .css_classes(["pill"])
            .sensitive(false)
            .build();
        let reset = gtk::Button::builder()
            .label("Revenir au preset du profil")
            .css_classes(["pill"])
            .build();
        let restore = gtk::Button::builder()
            .label("Rendre la main à l'EC")
            .tooltip_text("Remet les courbes d'usine du constructeur")
            .css_classes(["pill", "destructive-action"])
            .visible(false)
            .build();
        let actions = gtk::FlowBox::builder()
            .selection_mode(gtk::SelectionMode::None)
            .max_children_per_line(4)
            .column_spacing(8)
            .row_spacing(8)
            .build();
        for b in [&apply, &revert, &reset, &restore] {
            actions.append(b);
        }
        editor.append(&actions);
        content.append(&glass(&editor));

        let boost = adw::SwitchRow::builder()
            .title("Cooler Boost")
            .subtitle("Ventilateurs au maximum, quelle que soit la courbe")
            .build();
        let boost_group = adw::PreferencesGroup::new();
        boost_group.add(&boost);
        content.append(&boost_group);

        let gate = Gate::new(ctx, Service::Daemon, &page_scroller(&content));
        let page = Rc::new(Self {
            gate,
            ctx: ctx.clone(),
            state: RefCell::default(),
            fan_switch,
            origin,
            chart,
            cells,
            report,
            report_list,
            error,
            error_label,
            params,
            thresholds,
            apply,
            revert,
            reset,
            restore,
            boost,
            syncing: Cell::new(false),
            debounce: Rc::default(),
        });
        page.connect();
        page
    }

    fn connect(self: &Rc<Self>) {
        let w = Rc::downgrade(self);
        self.fan_switch.connect_active_name_notify(move |g| {
            let (Some(p), Some(name)) = (w.upgrade(), g.active_name()) else {
                return;
            };
            let fan = if name == "gpu" { Fan::Gpu } else { Fan::Cpu };
            p.state.borrow_mut().fan = Some(fan);
            p.load();
        });
        for (&param, row) in &self.params {
            let w = Rc::downgrade(self);
            row.scale.connect_value_changed(move |s| {
                if let Some(p) = w.upgrade()
                    && !p.syncing.get()
                {
                    p.edit(param, s.value());
                }
            });
        }
        let w = Rc::downgrade(self);
        self.apply.connect_clicked(move |_| {
            if let Some(p) = w.upgrade() {
                p.apply_curve();
            }
        });
        let w = Rc::downgrade(self);
        self.revert.connect_clicked(move |_| {
            if let Some(p) = w.upgrade() {
                let spec = p.state.borrow().loaded.as_ref().map(|l| l.spec.clone());
                p.state.borrow_mut().edit = spec;
                p.refresh();
            }
        });
        let w = Rc::downgrade(self);
        self.reset.connect_clicked(move |_| {
            if let Some(p) = w.upgrade() {
                p.reset_curve();
            }
        });
        let w = Rc::downgrade(self);
        self.restore.connect_clicked(move |b| {
            if let Some(p) = w.upgrade() {
                p.confirm_restore(b);
            }
        });
        let w = Rc::downgrade(self);
        self.boost.connect_active_notify(move |s| {
            if let Some(p) = w.upgrade()
                && !p.syncing.get()
            {
                p.set_boost(s.is_active());
            }
        });
    }

    /// The daemon answered: (re)load the curve.
    pub fn daemon_up(self: &Rc<Self>, restore_factory: bool) {
        self.restore.set_visible(restore_factory);
        self.load();
    }

    /// (Re)loads the curve of the selected fan.
    pub fn load(self: &Rc<Self>) {
        let fan = self.state.borrow().fan();
        let w = Rc::downgrade(self);
        let ctx = self.ctx.clone();
        Ctx::spawn(async move {
            let res = ctx
                .app
                .backend
                .daemon(move |d| async move { d.fan_curve(fan).await })
                .await;
            let Some(p) = w.upgrade() else { return };
            match res {
                Ok(c) => {
                    let mut s = p.state.borrow_mut();
                    if s.fan() != fan {
                        return;
                    }
                    s.edit = Some(c.spec.clone());
                    s.loaded = Some(c);
                    s.preview = None;
                    s.error = None;
                    drop(s);
                    p.refresh();
                }
                Err(e) if !e.is_unavailable() => ctx.error(&e),
                Err(_) => {}
            }
        });
    }

    fn edit(self: &Rc<Self>, param: Param, v: f64) {
        let spec = {
            let mut s = self.state.borrow_mut();
            let Some(cur) = s.edit.clone() else { return };
            let next = param.set(&cur, v);
            s.edit = Some(next.clone());
            s.error = next.validate().err();
            s.preview = None;
            s.seq += 1;
            next
        };
        self.refresh();
        if self.state.borrow().error.is_some() || !self.state.borrow().dirty() {
            return;
        }
        let seq = self.state.borrow().seq;
        let w = Rc::downgrade(self);
        self.debounce.run(140, move || {
            let Some(p) = w.upgrade() else { return };
            let ctx = p.ctx.clone();
            let w = Rc::downgrade(&p);
            Ctx::spawn(async move {
                let res = ctx
                    .app
                    .backend
                    .daemon(move |d| async move { d.preview_fan_curve(&spec).await })
                    .await;
                let Some(p) = w.upgrade() else { return };
                if p.state.borrow().seq != seq {
                    return;
                }
                match res {
                    Ok(g) => p.state.borrow_mut().preview = Some(g),
                    Err(e) => p.state.borrow_mut().error = Some(e.message_fr()),
                }
                p.refresh();
            });
        });
    }

    fn apply_curve(self: &Rc<Self>) {
        let (fan, Some(spec)) = ({
            let s = self.state.borrow();
            (s.fan(), s.edit.clone())
        }) else {
            return;
        };
        self.apply.set_sensitive(false);
        let w = Rc::downgrade(self);
        let ctx = self.ctx.clone();
        Ctx::spawn(async move {
            let res = ctx
                .app
                .backend
                .daemon(move |d| async move { d.set_fan_curve(fan, &spec).await })
                .await;
            match res {
                Ok(g) if g.raised.is_empty() => ctx.toast("Courbe appliquée."),
                Ok(g) => ctx.toast(&format!(
                    "Courbe appliquée ; {} palier(s) relevé(s) par la sécurité S2.",
                    g.raised.len()
                )),
                Err(e) => ctx.error(&e),
            }
            if let Some(p) = w.upgrade() {
                p.load();
            }
        });
    }

    fn reset_curve(self: &Rc<Self>) {
        let fan = self.state.borrow().fan();
        let w = Rc::downgrade(self);
        let ctx = self.ctx.clone();
        Ctx::spawn(async move {
            match ctx
                .app
                .backend
                .daemon(move |d| async move { d.reset_fan_curve(fan).await })
                .await
            {
                Ok(()) => ctx.toast("Preset du profil rétabli."),
                Err(e) => ctx.error(&e),
            }
            if let Some(p) = w.upgrade() {
                p.load();
            }
        });
    }

    fn confirm_restore(self: &Rc<Self>, anchor: &gtk::Button) {
        let dialog = adw::AlertDialog::new(
            Some("Rendre la main à l'EC ?"),
            Some(
                "Les ventilateurs suivront de nouveau les courbes d'usine du constructeur. Vos courbes \
                 restent enregistrées et reviennent au prochain changement de profil.",
            ),
        );
        dialog.add_response("cancel", "Annuler");
        dialog.add_response("restore", "Rendre la main");
        dialog.set_response_appearance("restore", adw::ResponseAppearance::Destructive);
        dialog.set_default_response(Some("cancel"));
        dialog.set_close_response("cancel");
        let ctx = self.ctx.clone();
        let w = Rc::downgrade(self);
        let anchor = anchor.clone();
        Ctx::spawn(async move {
            if dialog.choose_future(Some(&anchor)).await != "restore" {
                return;
            }
            match ctx
                .app
                .backend
                .daemon(|d| async move { d.restore_factory().await })
                .await
            {
                Ok(()) => ctx.toast("L'EC a repris la main (courbes d'usine)."),
                Err(e) => ctx.error(&e),
            }
            if let Some(p) = w.upgrade() {
                p.load();
            }
        });
    }

    fn set_boost(&self, on: bool) {
        let ctx = self.ctx.clone();
        Ctx::spawn(async move {
            match ctx
                .app
                .backend
                .daemon(move |d| async move { d.set_cooler_boost(on).await })
                .await
            {
                Ok(()) => ctx.toast(if on {
                    "Cooler Boost activé."
                } else {
                    "Cooler Boost désactivé."
                }),
                Err(e) => {
                    ctx.error(&e);
                    if let Some(w) = ctx.app.window.borrow().as_ref()
                        && let Some(s) = ctx.app.shared.borrow().status.as_ref()
                    {
                        w.fans.set_status(s);
                    }
                }
            }
        });
    }

    /// Daemon status changed (profile, boost, read-only).
    pub fn set_status(self: &Rc<Self>, s: &DaemonStatus) {
        let profile_changed = self
            .state
            .borrow()
            .status
            .as_ref()
            .is_some_and(|old| old.profile != s.profile);
        self.state.borrow_mut().status = Some(s.clone());
        self.syncing.set(true);
        self.boost.set_active(s.cooler_boost);
        self.syncing.set(false);
        self.boost.set_sensitive(!s.boost_forced && !s.read_only);
        self.boost.set_subtitle(if s.boost_forced {
            "Forcé par la sécurité thermique (S4) : il se relâche quand CPU et GPU repassent sous 85 °C"
        } else {
            "Ventilateurs au maximum, quelle que soit la courbe"
        });
        if profile_changed {
            self.load();
        } else {
            self.refresh();
        }
    }

    /// New telemetry: moves the live marker. When the CPU fan is driven live (ADR-0010) and its target moved,
    /// reloads the curve so the chart and table show the vector in force.
    pub fn set_telemetry(self: &Rc<Self>, t: &Telemetry) {
        let (fan, stale) = {
            let mut s = self.state.borrow_mut();
            let previous = s.temps.cpu_fan_target;
            s.temps = t.clone();
            let live = s
                .loaded
                .as_ref()
                .filter(|l| l.generated.table.is_live_cpu())
                .map(|l| l.generated.table.speeds[0]);
            let stale = s.fan() == Fan::Cpu
                && !s.dirty()
                && t.cpu_fan_target.is_some_and(|target| live != Some(target))
                && t.cpu_fan_target != previous;
            (s.fan(), stale)
        };
        if stale {
            self.load();
            return;
        }
        let temp = t.temp_of(fan);
        self.chart.update(|d| d.temp = temp);
        if fan == Fan::Cpu {
            self.refresh();
        } else {
            self.refresh_table();
        }
    }

    fn refresh(&self) {
        let s = self.state.borrow();
        let fan = s.fan();
        let read_only = s.status.as_ref().is_some_and(|st| st.read_only);
        let profile = s
            .status
            .as_ref()
            .map_or("?", |st| model::profile_label(&st.profile));
        let origin = match &s.loaded {
            Some(l) if l.custom => format!("Courbe personnalisée du profil {profile}."),
            Some(_) => format!("Preset du profil {profile}."),
            None => "Chargement…".to_owned(),
        };
        let dirty = s.dirty();
        let extra = if dirty {
            " Aperçu non appliqué (pointillés)."
        } else {
            ""
        };
        let live = fan == Fan::Cpu && s.loaded.as_ref().is_some_and(|l| l.generated.table.is_live_cpu());
        let live_note = if live {
            format!(" {}", live_cpu_note(s.temps.cpu_fan_target))
        } else {
            String::new()
        };
        self.origin
            .set_label(&format!("{} — {origin}{extra}{live_note}", fan.label()));

        // Sliders.
        if let Some(e) = &s.edit {
            self.syncing.set(true);
            for (&p, row) in &self.params {
                let v = p.get(e);
                row.scale.set_value(v);
                row.value.set_label(&p.format(v));
                row.scale.set_sensitive(!read_only);
            }
            self.syncing.set(false);
            let up: Vec<String> = e.up.iter().map(u8::to_string).collect();
            self.thresholds.set_label(&format!(
                "Seuils de montée : {} {NNBSP}°C (répartis entre T0 et T1)",
                up.join(" · ")
            ));
        }
        if let Some(err) = &s.error {
            self.error_label.set_label(err);
        }
        self.error.set_reveal_child(s.error.is_some());
        self.apply
            .set_sensitive(dirty && s.error.is_none() && s.preview.is_some() && !read_only);
        self.revert.set_sensitive(dirty);
        self.reset
            .set_sensitive(!read_only && s.loaded.as_ref().is_some_and(|l| l.custom));

        // Report.
        while let Some(c) = self.report_list.first_child() {
            self.report_list.remove(&c);
        }
        let shown = s.shown().cloned();
        if let Some(g) = &shown {
            for r in &g.raised {
                self.report_list.append(
                    &gtk::Label::builder()
                        .label(format!("▲ {}", r.describe()))
                        .xalign(0.0)
                        .wrap(true)
                        .build(),
                );
            }
        }
        self.report
            .set_reveal_child(shown.as_ref().is_some_and(|g| !g.raised.is_empty()));
        let applied = s.loaded.as_ref().map(|l| l.generated.clone());
        let preview = if dirty { s.preview.clone() } else { None };
        let spec = s.edit.clone();
        let temp = s.temps.temp_of(fan);
        drop(s);
        self.chart.update(|d| {
            d.applied = applied;
            d.preview = preview;
            d.spec = spec;
            d.temp = temp;
        });
        self.refresh_table();
    }

    fn refresh_table(&self) {
        let s = self.state.borrow();
        let Some(g) = s.shown() else {
            for row in &self.cells {
                for l in row {
                    l.set_label(format::UNKNOWN);
                }
            }
            return;
        };
        let t = &g.table;
        let current = s.temps.temp_of(s.fan()).map(|temp| chart::marker(t, temp).step);
        for i in 0..7 {
            let is_current = current == Some(i);
            let step = if is_current {
                format!("{i} ●")
            } else {
                i.to_string()
            };
            let cols = [
                step,
                format!("{}{NNBSP}%", t.speeds[i]),
                if i == 0 {
                    "–".to_owned()
                } else {
                    format!("{}°", t.up[i - 1])
                },
                if i == 0 {
                    "–".to_owned()
                } else {
                    format!("{}°", t.down[i - 1])
                },
                if g.is_raised(i) {
                    "▲ relevé".to_owned()
                } else {
                    "–".to_owned()
                },
            ];
            for (row, text) in self.cells.iter().zip(cols) {
                let l = &row[i];
                l.set_label(&text);
                if g.is_raised(i) {
                    l.add_css_class("raised");
                } else {
                    l.remove_css_class("raised");
                }
                if is_current {
                    l.add_css_class("current");
                } else {
                    l.remove_css_class("current");
                }
            }
        }
    }
}
