//! Accueil: the profile switch (hero), live ring gauges, status at a glance.

use std::rc::Rc;

use adw::prelude::*;
use msi_center::chart::gauge_fraction;
use msi_center::errors::Service;
use msi_center::format;
use msi_center::model::{self, DaemonStatus, Telemetry, TempLevel};
use msi_core::fan_profile::FanProfile;
use msi_core::profile::Profile;

use super::profiles::{ProfileSwitch, Variant};
use super::widgets::{Ctx, Gate, Gauge, chip, glass, page_header, page_scroller, section_title};

/// Highest rpm on the gauges (100 % is ≈ 6 000 tr/min on this machine; values above are clamped).
const RPM_MAX: f64 = 6500.0;

/// The page.
#[derive(Debug)]
pub struct Home {
    /// Root widget.
    pub gate: Gate,
    /// The hero profile switch.
    pub profiles: ProfileSwitch,
    profile_desc: gtk::Label,
    gpu_warning: gtk::Revealer,
    gpu_warning_text: gtk::Label,
    power: gtk::Label,
    fan_chip: gtk::Button,
    fan_icon: gtk::Image,
    fan_label: gtk::Label,
    boost: gtk::Label,
    dgpu_chip: gtk::Label,
    read_only: std::cell::Cell<bool>,
    cpu_temp: Gauge,
    gpu_temp: Gauge,
    fan1: Gauge,
    fan2: Gauge,
    cpu: Gauge,
    dgpu: Gauge,
    igpu: Gauge,
    ram: Gauge,
    npu: Gauge,
    power_gauge: Gauge,
}

fn flow(gauges: &[&Gauge]) -> gtk::FlowBox {
    let f = gtk::FlowBox::builder()
        .selection_mode(gtk::SelectionMode::None)
        .homogeneous(true)
        .min_children_per_line(2)
        .max_children_per_line(6)
        .column_spacing(14)
        .row_spacing(14)
        .build();
    for g in gauges {
        f.append(&g.root);
    }
    f
}

impl Home {
    /// Builds the page.
    #[allow(clippy::too_many_lines, reason = "widget tree")]
    pub fn new(ctx: &Rc<Ctx>) -> Rc<Self> {
        let content = gtk::Box::new(gtk::Orientation::Vertical, 16);
        content.append(&page_header("Accueil", "MSI Stealth 14 AI Studio · en direct"));

        // Hero: the profile switch. One click applies.
        let app = Rc::downgrade(&ctx.app);
        let profiles = ProfileSwitch::new(Variant::Hero, "Profil", move |p| {
            if let Some(a) = app.upgrade() {
                a.apply_profile(p);
            }
        });
        let head = gtk::Box::new(gtk::Orientation::Horizontal, 12);
        head.append(
            &gtk::Label::builder()
                .label("Profil")
                .xalign(0.0)
                .hexpand(true)
                .css_classes(["card-heading"])
                .accessible_role(gtk::AccessibleRole::Heading)
                .build(),
        );
        let chips = adw::WrapBox::builder()
            .child_spacing(6)
            .line_spacing(6)
            .halign(gtk::Align::End)
            .build();
        let (c, power) = chip("battery-full-charged-symbolic");
        chips.append(&c);
        // Fan profile next to the performance profile: a chip that opens Ventilateurs.
        let fan_icon = gtk::Image::from_icon_name("weather-windy-symbolic");
        let fan_label = gtk::Label::builder().css_classes(["numeric"]).build();
        let fan_box = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        fan_box.append(&fan_icon);
        fan_box.append(&fan_label);
        fan_box.append(&gtk::Image::from_icon_name("go-next-symbolic"));
        let fan_chip = gtk::Button::builder()
            .child(&fan_box)
            .css_classes(["chip", "chip-button"])
            .action_name("win.page")
            .action_target(&"fans".to_variant())
            .build();
        chips.append(&fan_chip);
        let (c, boost) = chip("temperature-symbolic");
        chips.append(&c);
        let (c, dgpu_chip) = chip("video-display-symbolic");
        chips.append(&c);
        head.append(&chips);

        let profile_desc = gtk::Label::builder()
            .xalign(0.0)
            .wrap(true)
            .css_classes(["dim", "caption"])
            .build();

        let gpu_warning_text = gtk::Label::builder().xalign(0.0).wrap(true).hexpand(true).build();
        let see = gtk::Button::builder()
            .label("Voir les processus")
            .valign(gtk::Align::Center)
            .css_classes(["pill", "small"])
            .action_name("win.page")
            .action_target(&"perf".to_variant())
            .build();
        let warn = gtk::Box::new(gtk::Orientation::Horizontal, 10);
        warn.add_css_class("inline-warning");
        warn.append(&gtk::Image::from_icon_name("dialog-warning-symbolic"));
        warn.append(&gpu_warning_text);
        warn.append(&see);
        let gpu_warning = gtk::Revealer::builder()
            .transition_type(gtk::RevealerTransitionType::SlideDown)
            .transition_duration(200)
            .child(&warn)
            .build();

        let hero = gtk::Box::new(gtk::Orientation::Vertical, 14);
        hero.append(&head);
        hero.append(&profiles.root);
        hero.append(&gpu_warning);
        hero.append(&profile_desc);
        let card = glass(&hero);
        card.add_css_class("hero-card");
        content.append(&card);

        let cpu_temp = Gauge::new("CPU", "temperature-symbolic");
        let gpu_temp = Gauge::new("GPU", "temperature-symbolic");
        let fan1 = Gauge::new("Ventilateur CPU", "weather-windy-symbolic");
        let fan2 = Gauge::new("Ventilateur GPU", "weather-windy-symbolic");
        content.append(&section_title("Températures et ventilateurs"));
        content.append(&flow(&[&cpu_temp, &gpu_temp, &fan1, &fan2]));

        let cpu = Gauge::new("Processeur", "computer-symbolic");
        let dgpu = Gauge::new("RTX 4070", "video-display-symbolic");
        let igpu = Gauge::new("Arc (iGPU)", "video-display-symbolic");
        let ram = Gauge::new("Mémoire", "drive-harddisk-solidstate-symbolic");
        let npu = Gauge::new("NPU", "applications-science-symbolic");
        let power_gauge = Gauge::new("Puissance", "battery-full-charged-symbolic");
        content.append(&section_title("Utilisation"));
        content.append(&flow(&[&cpu, &dgpu, &igpu, &ram, &npu, &power_gauge]));

        let gate = Gate::new(ctx, Service::Daemon, &page_scroller(&content));
        let page = Rc::new(Self {
            gate,
            profiles,
            profile_desc,
            gpu_warning,
            gpu_warning_text,
            power,
            fan_chip,
            fan_icon,
            fan_label,
            boost,
            dgpu_chip,
            read_only: std::cell::Cell::new(false),
            cpu_temp,
            gpu_temp,
            fan1,
            fan2,
            cpu,
            dgpu,
            igpu,
            ram,
            npu,
            power_gauge,
        });
        page.show_profile(None, false);
        page.show_fan_profile(None, false);
        page.set_telemetry(&Telemetry::default());
        page
    }

    /// Shows the selected (or being applied) profile.
    pub fn show_profile(&self, shown: Option<Profile>, pending: bool) {
        self.profiles.set(shown, pending, !self.read_only.get());
        self.profile_desc
            .set_label(&shown.map_or_else(String::new, model::profile_description));
    }

    /// Shows the fan profile of the selected profile in its chip.
    pub fn show_fan_profile(&self, shown: Option<FanProfile>, pending: bool) {
        self.fan_chip.set_visible(shown.is_some());
        let Some(fp) = shown else { return };
        self.fan_icon.set_icon_name(Some(model::fan_profile_icon(fp)));
        let dots = if pending { "…" } else { "" };
        self.fan_label
            .set_label(&format!("Ventilation · {}{dots}", fp.label()));
        let tip = format!(
            "Profil de ventilation : {} — {}. Ouvrir Ventilateurs",
            fp.label(),
            fp.description()
        );
        self.fan_chip.set_tooltip_text(Some(&tip));
        self.fan_chip
            .update_property(&[gtk::accessible::Property::Label(&tip)]);
    }

    /// The dGPU could not be removed (Éco): explain inline, with a way to the holders.
    pub fn show_gpu_busy(&self, text: &str) {
        self.gpu_warning_text.set_label(text);
        self.gpu_warning.set_reveal_child(true);
    }

    /// Daemon status changed.
    pub fn set_status(&self, s: &DaemonStatus) {
        self.read_only.set(s.read_only);
        if s.gpu_state == "removed" || s.profile != "eco" {
            self.gpu_warning.set_reveal_child(false);
        }
        let boost = if s.boost_forced {
            "Cooler Boost forcé"
        } else if s.cooler_boost {
            "Cooler Boost"
        } else {
            "Boost désactivé"
        };
        self.power.set_label(model::power_source_label(s.on_ac_power));
        self.boost.set_label(boost);
        self.dgpu_chip
            .set_label(&format!("RTX 4070 · {}", model::gpu_state_label(&s.gpu_state)));
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
        self.power_gauge.set(
            &format::watts(t.power_w),
            t.power_w.map(|w| gauge_fraction(w, 0.0, 115.0)),
            "processeur",
            None,
        );
    }
}
