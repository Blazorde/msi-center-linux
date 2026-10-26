//! Batterie: charge limit 50–100 % with an explanation, power source.

use std::cell::Cell;
use std::rc::Rc;

use adw::prelude::*;
use msi_center::errors::Service;
use msi_center::format;
use msi_center::model::{self, DaemonStatus};
use msi_core::battery::CHARGE_LIMIT_RANGE;

use super::widgets::{Ctx, Gate, glass, hint, page_scroller};

/// The page.
#[derive(Debug)]
pub struct Battery {
    /// Root widget.
    pub gate: Gate,
    ctx: Rc<Ctx>,
    stack: gtk::Stack,
    value: gtk::Label,
    scale: gtk::Scale,
    apply: gtk::Button,
    current: Cell<u8>,
    source: adw::ActionRow,
    limit_row: adw::ActionRow,
    read_only: Cell<bool>,
}

fn advice(limit: u8) -> &'static str {
    match limit {
        0..=65 => "Longévité maximale : idéal si l'ordinateur reste branché presque tout le temps.",
        66..=85 => "Bon compromis : la batterie vieillit nettement moins vite qu'à 100 %.",
        86..=99 => "Autonomie presque complète, usure un peu réduite.",
        _ => "Charge complète : autonomie maximale, usure la plus rapide.",
    }
}

impl Battery {
    /// Builds the page.
    pub fn new(ctx: &Rc<Ctx>) -> Rc<Self> {
        let content = gtk::Box::new(gtk::Orientation::Vertical, 18);
        let (lo, hi) = (
            f64::from(*CHARGE_LIMIT_RANGE.start()),
            f64::from(*CHARGE_LIMIT_RANGE.end()),
        );
        let value = gtk::Label::builder()
            .css_classes(["big-value"])
            .xalign(0.0)
            .build();
        let scale = gtk::Scale::with_range(gtk::Orientation::Horizontal, lo, hi, 1.0);
        scale.set_draw_value(false);
        scale.set_hexpand(true);
        for m in [60.0, 80.0, 100.0] {
            scale.add_mark(m, gtk::PositionType::Bottom, Some(&format!("{m:.0} %")));
        }
        scale.update_property(&[gtk::accessible::Property::Label("Limite de charge")]);
        let apply = gtk::Button::builder()
            .label("Appliquer")
            .css_classes(["pill", "suggested-action"])
            .halign(gtk::Align::Start)
            .sensitive(false)
            .build();
        let advice_label = hint(advice(100));
        let hero = gtk::Box::new(gtk::Orientation::Vertical, 10);
        hero.append(&gtk::Box::builder().css_classes(["sunset-strip"]).build());
        hero.append(
            &gtk::Label::builder()
                .label("Limite de charge")
                .xalign(0.0)
                .css_classes(["title-2"])
                .accessible_role(gtk::AccessibleRole::Heading)
                .build(),
        );
        hero.append(&value);
        hero.append(&scale);
        hero.append(&advice_label);
        hero.append(&apply);
        content.append(&glass(&hero));

        let why = gtk::Box::new(gtk::Orientation::Vertical, 8);
        why.append(
            &gtk::Label::builder()
                .label("Pourquoi limiter la charge ?")
                .xalign(0.0)
                .css_classes(["card-title"])
                .build(),
        );
        why.append(&hint(
            "Une batterie lithium-ion s'use surtout quand elle reste longtemps pleine et chaude. \
             S'arrêter à 80 % quand l'ordinateur est souvent branché ralentit fortement cette usure \
             (plus de cycles avant de perdre de la capacité). La limite est appliquée par le \
             firmware : la batterie cesse de charger au seuil choisi, même éteint. Remontez à 100 % \
             la veille d'un déplacement.",
        ));
        content.append(&glass(&why));

        let rows = adw::PreferencesGroup::new();
        let source = adw::ActionRow::builder().title("Source d'alimentation").build();
        source.add_css_class("property");
        let limit_row = adw::ActionRow::builder().title("Limite appliquée").build();
        limit_row.add_css_class("property");
        rows.add(&source);
        rows.add(&limit_row);
        content.append(&rows);

        let unsupported = adw::StatusPage::builder()
            .icon_name("battery-missing-symbolic")
            .title("Limite de charge non prise en charge")
            .description("Le pilote n'expose pas charge_control_end_threshold sur cette machine.")
            .build();
        let stack = gtk::Stack::new();
        stack.add_named(&page_scroller(&content), Some("ok"));
        stack.add_named(&unsupported, Some("unsupported"));

        let gate = Gate::new(ctx, Service::Daemon, &stack);
        let page = Rc::new(Self {
            gate,
            ctx: ctx.clone(),
            stack,
            value,
            scale,
            apply,
            current: Cell::new(0),
            source,
            limit_row,
            read_only: Cell::new(false),
        });
        let w = Rc::downgrade(&page);
        page.scale.connect_value_changed(move |s| {
            let Some(p) = w.upgrade() else { return };
            let v = to_u8(s.value());
            p.value.set_label(&format::pct_u8(v));
            advice_label.set_label(advice(v));
            p.apply.set_sensitive(v != p.current.get() && !p.read_only.get());
        });
        let w = Rc::downgrade(&page);
        page.apply.connect_clicked(move |_| {
            if let Some(p) = w.upgrade() {
                p.apply_limit(to_u8(p.scale.value()));
            }
        });
        page
    }

    fn apply_limit(&self, limit: u8) {
        self.apply.set_sensitive(false);
        let ctx = self.ctx.clone();
        Ctx::spawn(async move {
            match ctx
                .app
                .backend
                .daemon(move |d| async move { d.set_charge_limit(limit).await })
                .await
            {
                Ok(()) => ctx.toast(&format!("Limite de charge réglée à {}.", format::pct_u8(limit))),
                Err(e) => {
                    ctx.error(&e);
                    if let Some(w) = ctx.app.window.borrow().as_ref()
                        && let Some(s) = ctx.app.shared.borrow().status.as_ref()
                    {
                        w.battery.set_status(s);
                    }
                }
            }
        });
    }

    /// Daemon status changed.
    pub fn set_status(&self, s: &DaemonStatus) {
        self.read_only.set(s.read_only);
        self.source.set_subtitle(model::power_source_label(s.on_ac_power));
        if s.charge_limit == 0 {
            self.stack.set_visible_child_name("unsupported");
            return;
        }
        self.stack.set_visible_child_name("ok");
        self.limit_row.set_subtitle(&format::pct_u8(s.charge_limit));
        let changed = self.current.replace(s.charge_limit) != s.charge_limit;
        if changed {
            self.scale.set_value(f64::from(s.charge_limit));
        }
        self.value.set_label(&format::pct_u8(to_u8(self.scale.value())));
        self.apply
            .set_sensitive(to_u8(self.scale.value()) != s.charge_limit && !s.read_only);
    }
}

#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "slider range is 50..=100"
)]
fn to_u8(v: f64) -> u8 {
    v.round().clamp(0.0, 255.0) as u8
}
