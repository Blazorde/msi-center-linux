//! Performance & GPU: profile details, dGPU state and holders, automatic profile on AC.

use std::cell::Cell;
use std::rc::Rc;

use adw::prelude::*;
use msi_center::errors::Service;
use msi_center::model::{self, DaemonStatus};
use msi_core::profile::Profile;

use super::widgets::{Ctx, Gate, page_scroller};

/// The page.
#[derive(Debug)]
pub struct Perf {
    /// Root widget.
    pub gate: Gate,
    ctx: Rc<Ctx>,
    profile_rows: Vec<(Profile, adw::ActionRow, gtk::Image)>,
    gpu_state: adw::ActionRow,
    holders: adw::PreferencesGroup,
    holder_rows: std::cell::RefCell<Vec<adw::ActionRow>>,
    auto: adw::SwitchRow,
    source: adw::ActionRow,
    fan_mode: adw::ActionRow,
    firmware: adw::ActionRow,
    read_only: adw::ActionRow,
    syncing: Cell<bool>,
}

fn value_row(title: &str) -> adw::ActionRow {
    let r = adw::ActionRow::builder().title(title).use_markup(false).build();
    r.add_css_class("property");
    r
}

impl Perf {
    /// Builds the page.
    pub fn new(ctx: &Rc<Ctx>) -> Rc<Self> {
        let content = gtk::Box::new(gtk::Orientation::Vertical, 18);

        let profiles = adw::PreferencesGroup::builder()
            .title("Profils")
            .description("Ce que chaque profil applique. Changez de profil depuis l'Accueil ou l'extension.")
            .build();
        let mut profile_rows = Vec::new();
        for p in Profile::ALL {
            let row = adw::ActionRow::builder()
                .title(model::profile_label(p.as_str()))
                .subtitle(model::profile_description(p))
                .subtitle_lines(3)
                .build();
            let check = gtk::Image::builder()
                .icon_name("object-select-symbolic")
                .tooltip_text("Profil actif")
                .visible(false)
                .build();
            check.update_property(&[gtk::accessible::Property::Label("Profil actif")]);
            row.add_suffix(&check);
            profiles.add(&row);
            profile_rows.push((p, row, check));
        }
        content.append(&profiles);

        let gpu = adw::PreferencesGroup::builder()
            .title("Carte graphique NVIDIA (RTX 4070)")
            .build();
        let gpu_state = value_row("État");
        gpu.add(&gpu_state);
        content.append(&gpu);

        let refresh = gtk::Button::builder()
            .icon_name("view-refresh-symbolic")
            .tooltip_text("Actualiser la liste")
            .valign(gtk::Align::Center)
            .css_classes(["flat"])
            .build();
        let holders = adw::PreferencesGroup::builder()
            .title("Processus qui la maintiennent éveillée")
            .description(
                "En Éco, la carte n'est retirée que si aucun processus ne l'utilise. Rien n'est jamais \
                 tué : fermez ces applications pour l'éteindre.",
            )
            .header_suffix(&refresh)
            .build();
        content.append(&holders);

        let power = adw::PreferencesGroup::builder().title("Alimentation").build();
        let auto = adw::SwitchRow::builder()
            .title("Profil automatique secteur / batterie")
            .subtitle("Balanced sur secteur, Éco sur batterie")
            .build();
        let source = value_row("Source actuelle");
        power.add(&auto);
        power.add(&source);
        content.append(&power);

        let system = adw::PreferencesGroup::builder()
            .title("Contrôleur embarqué (EC)")
            .build();
        let fan_mode = value_row("Pilotage des ventilateurs");
        let firmware = value_row("Firmware EC");
        let read_only = value_row("Écriture");
        system.add(&fan_mode);
        system.add(&firmware);
        system.add(&read_only);
        content.append(&system);

        let gate = Gate::new(ctx, Service::Daemon, &page_scroller(&content));
        let page = Rc::new(Self {
            gate,
            ctx: ctx.clone(),
            profile_rows,
            gpu_state,
            holders,
            holder_rows: std::cell::RefCell::default(),
            auto,
            source,
            fan_mode,
            firmware,
            read_only,
            syncing: Cell::new(false),
        });
        let w = Rc::downgrade(&page);
        refresh.connect_clicked(move |_| {
            if let Some(p) = w.upgrade() {
                p.load_holders();
            }
        });
        let w = Rc::downgrade(&page);
        page.auto.connect_active_notify(move |s| {
            if let Some(p) = w.upgrade()
                && !p.syncing.get()
            {
                p.set_auto(s.is_active());
            }
        });
        page
    }

    fn set_auto(&self, on: bool) {
        let ctx = self.ctx.clone();
        Ctx::spawn(async move {
            match ctx
                .app
                .backend
                .daemon(move |d| async move { d.set_auto_profile_on_power(on).await })
                .await
            {
                Ok(()) => ctx.toast(if on {
                    "Profil automatique activé."
                } else {
                    "Profil automatique désactivé."
                }),
                Err(e) => {
                    ctx.error(&e);
                    if let Some(w) = ctx.app.window.borrow().as_ref()
                        && let Some(s) = ctx.app.shared.borrow().status.as_ref()
                    {
                        w.perf.set_status(s);
                    }
                }
            }
        });
    }

    /// Reloads `GpuHolders`.
    pub fn load_holders(self: &Rc<Self>) {
        let w = Rc::downgrade(self);
        let ctx = self.ctx.clone();
        Ctx::spawn(async move {
            let res = ctx
                .app
                .backend
                .daemon(|d| async move { d.gpu_holders().await })
                .await;
            let Some(p) = w.upgrade() else { return };
            match res {
                Ok(list) => p.show_holders(&list),
                Err(e) if !e.is_unavailable() => ctx.error(&e),
                Err(_) => {}
            }
        });
    }

    fn show_holders(&self, list: &[(u32, String)]) {
        for r in self.holder_rows.borrow_mut().drain(..) {
            self.holders.remove(&r);
        }
        let mut rows = Vec::new();
        if list.is_empty() {
            rows.push(
                adw::ActionRow::builder()
                    .title("Aucun processus")
                    .subtitle("La carte peut être mise en veille ou retirée.")
                    .build(),
            );
        }
        for (pid, name) in list {
            let r = adw::ActionRow::builder()
                .use_markup(false)
                .title(name.as_str())
                .subtitle(format!("PID {pid}"))
                .build();
            r.add_prefix(&gtk::Image::from_icon_name("application-x-executable-symbolic"));
            rows.push(r);
        }
        for r in &rows {
            self.holders.add(r);
        }
        *self.holder_rows.borrow_mut() = rows;
    }

    /// Daemon status changed.
    pub fn set_status(self: &Rc<Self>, s: &DaemonStatus) {
        for (p, row, check) in &self.profile_rows {
            let active = p.as_str() == s.profile;
            check.set_visible(active);
            if active {
                row.add_css_class("accent");
            } else {
                row.remove_css_class("accent");
            }
        }
        let previous = self.gpu_state.subtitle();
        self.gpu_state.set_subtitle(model::gpu_state_label(&s.gpu_state));
        if previous.as_deref() != Some(model::gpu_state_label(&s.gpu_state)) {
            self.load_holders();
        }
        self.syncing.set(true);
        self.auto.set_active(s.auto_profile_on_power);
        self.syncing.set(false);
        self.auto.set_sensitive(!s.read_only);
        self.source.set_subtitle(model::power_source_label(s.on_ac_power));
        self.fan_mode.set_subtitle(model::fan_mode_label(&s.fan_mode));
        self.firmware.set_subtitle(if s.firmware_version.is_empty() {
            "inconnu (module msi-ec absent ?)"
        } else {
            &s.firmware_version
        });
        if s.read_only {
            self.read_only
                .set_subtitle(&format!("Lecture seule : {}", s.read_only_reason));
        } else {
            self.read_only.set_subtitle("Autorisée (firmware reconnu)");
        }
    }
}
