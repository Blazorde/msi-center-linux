//! Accueil: live gauges, profile switch, at-a-glance status.

use std::cell::Cell;
use std::rc::Rc;

use adw::prelude::*;
use msi_center::chart::gauge_fraction;
use msi_center::errors::{AppError, Service};
use msi_center::format;
use msi_center::model::{self, DaemonStatus, Telemetry, TempLevel};
use msi_core::profile::Profile;

use super::widgets::{Ctx, Gate, Gauge, glass, hint, page_scroller, section_title};

/// Highest rpm on the gauges (100 % is ≈ 6 000 tr/min on this machine; values above are clamped).
const RPM_MAX: f64 = 6500.0;

/// The page.
#[derive(Debug)]
pub struct Home {
    /// Root widget.
    pub gate: Gate,
    ctx: Rc<Ctx>,
    profiles: adw::ToggleGroup,
    profile_desc: gtk::Label,
    status_line: gtk::Label,
    cpu_temp: Gauge,
    gpu_temp: Gauge,
    fan1: Gauge,
    fan2: Gauge,
    cpu: Gauge,
    dgpu: Gauge,
    igpu: Gauge,
    ram: Gauge,
    npu: Gauge,
    power: Gauge,
    syncing: Cell<bool>,
}

fn flow(gauges: &[&Gauge]) -> gtk::FlowBox {
    let f = gtk::FlowBox::builder()
        .selection_mode(gtk::SelectionMode::None)
        .homogeneous(true)
        .min_children_per_line(2)
        .max_children_per_line(6)
        .column_spacing(12)
        .row_spacing(12)
        .build();
    for g in gauges {
        f.append(&g.root);
    }
    f
}

impl Home {
    /// Builds the page.
    pub fn new(ctx: &Rc<Ctx>) -> Rc<Self> {
        let content = gtk::Box::new(gtk::Orientation::Vertical, 14);

        // Hero: profile switch.
        let profiles = adw::ToggleGroup::builder()
            .css_classes(["profile-switch"])
            .homogeneous(true)
            .halign(gtk::Align::Start)
            .build();
        for (p, icon) in [
            (Profile::Eco, "power-profile-power-saver-symbolic"),
            (Profile::Balanced, "power-profile-balanced-symbolic"),
            (Profile::Gaming, "power-profile-performance-symbolic"),
        ] {
            let label = model::profile_label(p.as_str());
            let content = adw::ButtonContent::builder().icon_name(icon).label(label).build();
            profiles.add(
                adw::Toggle::builder()
                    .name(p.as_str())
                    .child(&content)
                    .tooltip(model::profile_description(p))
                    .build(),
            );
        }
        let profile_desc = hint("");
        let status_line = gtk::Label::builder()
            .xalign(0.0)
            .wrap(true)
            .css_classes(["dim", "numeric"])
            .build();
        let hero = gtk::Box::new(gtk::Orientation::Vertical, 10);
        hero.append(&gtk::Box::builder().css_classes(["sunset-strip"]).build());
        hero.append(
            &gtk::Label::builder()
                .label("Profil")
                .xalign(0.0)
                .css_classes(["title-2"])
                .accessible_role(gtk::AccessibleRole::Heading)
                .build(),
        );
        hero.append(&profiles);
        hero.append(&profile_desc);
        hero.append(&status_line);
        content.append(&glass(&hero));

        let cpu_temp = Gauge::new("CPU", "temperature-symbolic");
        let gpu_temp = Gauge::new("GPU", "temperature-symbolic");
        let fan1 = Gauge::new("Ventilateur CPU", "weather-windy-symbolic");
        let fan2 = Gauge::new("Ventilateur GPU", "weather-windy-symbolic");
        content.append(&section_title("TEMPÉRATURES ET VENTILATEURS"));
        content.append(&flow(&[&cpu_temp, &gpu_temp, &fan1, &fan2]));

        let cpu = Gauge::new("Processeur", "computer-symbolic");
        let dgpu = Gauge::new("RTX 4070", "video-display-symbolic");
        let igpu = Gauge::new("Arc (iGPU)", "video-display-symbolic");
        let ram = Gauge::new("Mémoire", "drive-harddisk-solidstate-symbolic");
        let npu = Gauge::new("NPU", "applications-science-symbolic");
        let power = Gauge::new("Puissance", "battery-full-charged-symbolic");
        content.append(&section_title("UTILISATION"));
        content.append(&flow(&[&cpu, &dgpu, &igpu, &ram, &npu, &power]));

        let gate = Gate::new(ctx, Service::Daemon, &page_scroller(&content));
        let page = Rc::new(Self {
            gate,
            ctx: ctx.clone(),
            profiles,
            profile_desc,
            status_line,
            cpu_temp,
            gpu_temp,
            fan1,
            fan2,
            cpu,
            dgpu,
            igpu,
            ram,
            npu,
            power,
            syncing: Cell::new(false),
        });
        let weak = Rc::downgrade(&page);
        page.profiles.connect_active_name_notify(move |g| {
            let (Some(p), Some(name)) = (weak.upgrade(), g.active_name()) else {
                return;
            };
            if !p.syncing.get() {
                p.set_profile(name.to_string());
            }
        });
        page.set_telemetry(&Telemetry::default());
        page
    }

    fn set_profile(&self, name: String) {
        let ctx = self.ctx.clone();
        let label = model::profile_label(&name);
        let label = label.to_owned();
        Ctx::spawn(async move {
            let n = name.clone();
            match ctx
                .app
                .backend
                .daemon(move |d| async move { d.set_profile(&n).await })
                .await
            {
                Ok(()) => ctx.toast(&format!("Profil {label} appliqué.")),
                Err(e @ AppError::GpuBusy(_)) => {
                    ctx.error(&e);
                    if let Some(w) = ctx.app.window.borrow().as_ref() {
                        w.show_gpu_busy();
                    }
                }
                Err(e) => {
                    ctx.error(&e);
                    // Snap back to the real profile.
                    if let Some(w) = ctx.app.window.borrow().as_ref()
                        && let Some(s) = ctx.app.shared.borrow().status.as_ref()
                    {
                        w.home.set_status(s);
                    }
                }
            }
        });
    }

    /// Daemon status changed.
    pub fn set_status(&self, s: &DaemonStatus) {
        self.syncing.set(true);
        self.profiles.set_active_name(Some(&s.profile));
        self.syncing.set(false);
        for t in ["eco", "balanced", "gaming"] {
            if let Some(toggle) = self.profiles.toggle_by_name(t) {
                toggle.set_enabled(!s.read_only);
            }
        }
        let desc = match s.profile.as_str() {
            "eco" => model::profile_description(Profile::Eco),
            "gaming" => model::profile_description(Profile::Gaming),
            _ => model::profile_description(Profile::Balanced),
        };
        self.profile_desc.set_label(&desc);
        let boost = if s.boost_forced {
            "forcé (sécurité)"
        } else if s.cooler_boost {
            "activé"
        } else {
            "désactivé"
        };
        self.status_line.set_label(&format!(
            "Alimentation : {} · Ventilateurs : {} · Cooler Boost : {boost} · RTX 4070 : {}",
            model::power_source_label(s.on_ac_power),
            model::fan_mode_label(&s.fan_mode),
            model::gpu_state_label(&s.gpu_state),
        ));
    }

    /// New telemetry sample.
    pub fn set_telemetry(&self, t: &Telemetry) {
        let temp = |g: &Gauge, v: Option<f64>| {
            g.set(
                &format::temp(v),
                v.map(|c| gauge_fraction(c, 20.0, 100.0)),
                "",
                v.map(TempLevel::of),
            );
        };
        temp(&self.cpu_temp, t.cpu_temp);
        temp(&self.gpu_temp, t.gpu_temp);
        let fan = |g: &Gauge, rpm: Option<u32>, pct: Option<u8>| {
            let detail = pct.map_or_else(
                || "tr/min".to_owned(),
                |p| format!("tr/min · consigne {}", format::pct_u8(p)),
            );
            g.set(
                &rpm.map_or_else(
                    || format::UNKNOWN.to_owned(),
                    |r| format::group_thousands(u64::from(r)),
                ),
                rpm.map(|r| gauge_fraction(f64::from(r), 0.0, RPM_MAX)),
                &detail,
                None,
            );
        };
        fan(&self.fan1, t.fan1_rpm, t.fan1_pct);
        fan(&self.fan2, t.fan2_rpm, t.fan2_pct);
        let pct = |g: &Gauge, v: Option<f64>, detail: &str| {
            g.set(
                &format::pct(v),
                v.map(|p| gauge_fraction(p, 0.0, 100.0)),
                detail,
                None,
            );
        };
        pct(&self.cpu, t.cpu_pct, "");
        let dgpu_detail = t.dgpu_state.as_deref().map_or("", model::gpu_state_label);
        pct(&self.dgpu, t.dgpu_pct, dgpu_detail);
        pct(&self.igpu, t.igpu_pct, "");
        pct(&self.ram, t.ram_pct, "");
        pct(&self.npu, t.npu_pct, "");
        self.power.set(
            &format::watts(t.power_w),
            t.power_w.map(|w| gauge_fraction(w, 0.0, 115.0)),
            "processeur",
            None,
        );
    }
}
