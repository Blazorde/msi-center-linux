//! Main window: glass sidebar, page stack, warning banners, toasts.

use std::rc::Rc;

use adw::prelude::*;
use gtk::{gio, glib};
use msi_center::backend::Event;
use msi_center::dbus::DaemonEvent;
use msi_center::errors::Service;
use msi_center::model::{SafetyEvent, Severity};

use super::app::{App, Shared};
use super::backdrop;
use super::battery::Battery;
use super::clipboard::Clipboard;
use super::fans::Fans;
use super::home::Home;
use super::perf::Perf;
use super::prefs::Prefs;
use super::profiles::{ProfileSwitch, Variant};
use super::style;
use super::widgets::Ctx;

/// Pages: (id, title, icon).
const PAGES: [(&str, &str, &str); 6] = [
    ("home", "Accueil", "go-home-symbolic"),
    ("fans", "Ventilateurs", "weather-windy-symbolic"),
    ("perf", "Performance & GPU", "power-profile-performance-symbolic"),
    ("battery", "Batterie", "battery-full-symbolic"),
    ("clipboard", "Presse-papiers", "edit-paste-symbolic"),
    ("prefs", "Réglages", "emblem-system-symbolic"),
];

/// The window and its pages.
#[derive(Debug)]
pub struct Window {
    win: adw::ApplicationWindow,
    ctx: Rc<Ctx>,
    stack: gtk::Stack,
    sidebar: gtk::ListBox,
    split: adw::NavigationSplitView,
    content_page: adw::NavigationPage,
    daemon_banner: adw::Banner,
    read_only_banner: adw::Banner,
    gpu_banner: adw::Banner,
    safety_banner: adw::Banner,
    connection: gtk::Label,
    /// Compact profile switch of the header bar (hidden on Accueil, which has the large one).
    header_profiles: ProfileSwitch,
    /// Accueil.
    pub home: Rc<Home>,
    /// Ventilateurs.
    pub fans: Rc<Fans>,
    /// Performance & GPU.
    pub perf: Rc<Perf>,
    /// Batterie.
    pub battery: Rc<Battery>,
    /// Presse-papiers.
    pub clipboard: Rc<Clipboard>,
    /// Réglages.
    pub prefs: Rc<Prefs>,
}

fn sidebar_row(title: &str, icon: &str) -> gtk::ListBoxRow {
    let b = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    b.append(&gtk::Image::from_icon_name(icon));
    b.append(&gtk::Label::builder().label(title).xalign(0.0).build());
    gtk::ListBoxRow::builder().child(&b).build()
}

impl Window {
    /// Builds the window for `app`.
    #[allow(clippy::too_many_lines, reason = "widget tree")]
    pub fn new(app: &Rc<App>) -> Rc<Self> {
        let win = adw::ApplicationWindow::builder()
            .application(&app.gtk)
            .title("MSI Center")
            .default_width(1180)
            .default_height(800)
            .width_request(360)
            .height_request(420)
            .build();
        win.add_css_class("msi-window");
        let toasts = adw::ToastOverlay::new();
        let ctx = Rc::new(Ctx {
            app: app.clone(),
            toasts: toasts.clone(),
        });

        let home = Home::new(&ctx);
        let fans = Fans::new(&ctx);
        let perf = Perf::new(&ctx);
        let battery = Battery::new(&ctx);
        let clipboard = Clipboard::new(&ctx);
        let prefs = Prefs::new(&ctx);

        let stack = gtk::Stack::builder()
            .transition_type(gtk::StackTransitionType::Crossfade)
            .transition_duration(180)
            .vexpand(true)
            .build();
        stack.add_css_class("pages");
        stack.add_named(&home.gate.root, Some("home"));
        stack.add_named(&fans.gate.root, Some("fans"));
        stack.add_named(&perf.gate.root, Some("perf"));
        stack.add_named(&battery.gate.root, Some("battery"));
        stack.add_named(&clipboard.gate.root, Some("clipboard"));
        stack.add_named(&prefs.root, Some("prefs"));

        // Sidebar.
        let sidebar = gtk::ListBox::builder()
            .selection_mode(gtk::SelectionMode::Single)
            .css_classes(["navigation-sidebar"])
            .build();
        sidebar.update_property(&[gtk::accessible::Property::Label("Pages")]);
        for (_, title, icon) in PAGES {
            sidebar.append(&sidebar_row(title, icon));
        }
        let brand = gtk::Box::new(gtk::Orientation::Horizontal, 10);
        let mark = gtk::Box::builder()
            .css_classes(["brand-mark"])
            .valign(gtk::Align::Center)
            .build();
        let mark_icon = gtk::Image::from_icon_name("org.blazorde.MsiCenter-symbolic");
        mark_icon.set_hexpand(true);
        mark.set_halign(gtk::Align::Center);
        mark.set_hexpand(false);
        brand.set_hexpand(false);
        mark_icon.set_valign(gtk::Align::Center);
        mark.append(&mark_icon);
        brand.append(&mark);
        brand.append(
            &gtk::Label::builder()
                .label("MSI Center")
                .css_classes(["brand-title"])
                .build(),
        );
        let side_header = adw::HeaderBar::builder()
            .title_widget(&brand)
            .show_end_title_buttons(false)
            .build();
        let connection = gtk::Label::builder()
            .xalign(0.0)
            .wrap(true)
            .css_classes(["dim", "caption", "status-dot"])
            .margin_start(16)
            .margin_end(16)
            .margin_bottom(12)
            .build();
        let side_scroll = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vexpand(true)
            .child(&sidebar)
            .build();
        let side_box = gtk::Box::new(gtk::Orientation::Vertical, 0);
        side_box.append(&side_scroll);
        side_box.append(&connection);
        let side_view = adw::ToolbarView::new();
        side_view.add_top_bar(&side_header);
        side_view.set_content(Some(&side_box));
        let side_page = adw::NavigationPage::builder()
            .title("MSI Center")
            .child(&side_view)
            .build();

        // Content.
        let menu = gio::Menu::new();
        let profiles_menu = gio::Menu::new();
        for p in msi_core::profile::Profile::ALL {
            profiles_menu.append(
                Some(msi_center::model::profile_label(p.as_str())),
                Some(&format!(
                    "app.{}::{}",
                    msi_center::model::PROFILE_ACTION,
                    p.as_str()
                )),
            );
        }
        menu.append_section(Some("Profil"), &profiles_menu);
        let fans_menu = gio::Menu::new();
        for fp in msi_core::fan_profile::FanProfile::ALL {
            fans_menu.append(
                Some(fp.label()),
                Some(&format!(
                    "app.{}::{}",
                    msi_center::model::FAN_PROFILE_ACTION,
                    fp.as_str()
                )),
            );
        }
        menu.append_section(Some("Ventilation"), &fans_menu);
        menu.append(Some("Réglages"), Some("win.page::prefs"));
        menu.append(Some("À propos de MSI Center"), Some("app.about"));
        menu.append(Some("Quitter"), Some("app.quit"));
        let menu_button = gtk::MenuButton::builder()
            .icon_name("open-menu-symbolic")
            .menu_model(&menu)
            .tooltip_text("Menu principal")
            .primary(true)
            .build();
        let app_weak = Rc::downgrade(app);
        let header_profiles = ProfileSwitch::new(Variant::Compact, "Profil", move |p| {
            if let Some(a) = app_weak.upgrade() {
                a.apply_profile(p);
            }
        });
        let header = adw::HeaderBar::builder()
            .title_widget(&header_profiles.root)
            .build();
        header.pack_end(&menu_button);

        let daemon_banner = adw::Banner::builder()
            .title("Service msi-daemon injoignable : ventilateurs, profils et batterie indisponibles.")
            .button_label("Réessayer")
            .build();
        let read_only_banner = adw::Banner::new("");
        let gpu_banner = adw::Banner::builder()
            .title("La RTX 4070 est utilisée par des applications : elle reste allumée.")
            .button_label("Voir")
            .build();
        let safety_banner = adw::Banner::builder().button_label("OK").build();
        let content_box = gtk::Box::new(gtk::Orientation::Vertical, 0);
        for b in [&safety_banner, &daemon_banner, &read_only_banner, &gpu_banner] {
            // Titles carry service text (read-only reason): never parse it as markup.
            b.set_use_markup(false);
            content_box.append(b);
        }
        content_box.append(&stack);
        let content_view = adw::ToolbarView::new();
        content_view.add_top_bar(&header);
        content_view.set_content(Some(&content_box));
        content_view.add_css_class("content-pane");
        let content_page = adw::NavigationPage::builder()
            .title("Accueil")
            .child(&content_view)
            .build();

        let split = adw::NavigationSplitView::builder()
            .sidebar(&side_page)
            .content(&content_page)
            .min_sidebar_width(220.0)
            .max_sidebar_width(280.0)
            .build();
        toasts.set_child(Some(&split));

        // Backdrop under everything.
        let picture = gtk::Picture::builder()
            .content_fit(gtk::ContentFit::Cover)
            .can_target(false)
            .can_focus(false)
            .accessible_role(gtk::AccessibleRole::Presentation)
            .build();
        let overlay = gtk::Overlay::builder().child(&picture).build();
        overlay.add_overlay(&toasts);
        overlay.set_measure_overlay(&toasts, true);
        win.set_content(Some(&overlay));

        let bp = adw::Breakpoint::new(adw::BreakpointCondition::new_length(
            adw::BreakpointConditionLengthType::MaxWidth,
            720.0,
            adw::LengthUnit::Sp,
        ));
        bp.add_setter(&split, "collapsed", Some(&true.to_value()));
        win.add_breakpoint(bp);

        let me = Rc::new(Self {
            win,
            ctx,
            stack,
            sidebar,
            split,
            content_page,
            daemon_banner,
            read_only_banner,
            gpu_banner,
            safety_banner,
            connection,
            header_profiles,
            home,
            fans,
            perf,
            battery,
            clipboard,
            prefs,
        });
        me.connect(&picture);
        me.select("home");
        me
    }

    fn connect(self: &Rc<Self>, picture: &gtk::Picture) {
        let w = Rc::downgrade(self);
        self.sidebar.connect_row_selected(move |_, row| {
            let (Some(me), Some(row)) = (w.upgrade(), row) else {
                return;
            };
            let Some((id, title, _)) = usize::try_from(row.index()).ok().and_then(|i| PAGES.get(i)) else {
                return;
            };
            me.stack.set_visible_child_name(id);
            me.content_page.set_title(title);
            me.header_profiles.root.set_visible(*id != "home");
            me.split.set_show_content(true);
            if *id == "perf" {
                me.perf.load_holders();
            }
        });

        let page_action = gio::SimpleAction::new("page", Some(glib::VariantTy::STRING));
        let w = Rc::downgrade(self);
        page_action.connect_activate(move |_, v| {
            if let (Some(me), Some(id)) = (w.upgrade(), v.and_then(glib::Variant::get::<String>)) {
                me.select(&id);
            }
        });
        self.win.add_action(&page_action);

        let ctx = self.ctx.clone();
        self.daemon_banner.connect_button_clicked(move |_| {
            ctx.app.backend.retry(Service::Daemon);
            ctx.toast("Nouvelle tentative de connexion…");
        });
        let w = Rc::downgrade(self);
        self.gpu_banner.connect_button_clicked(move |b| {
            b.set_revealed(false);
            if let Some(me) = w.upgrade() {
                me.select("perf");
            }
        });
        self.safety_banner
            .connect_button_clicked(|b| b.set_revealed(false));

        // Backdrop: render after realize (needs the renderer) and again on light/dark change.
        let pic = picture.clone();
        picture.connect_realize(move |_| backdrop::paint(&pic, style::is_dark()));
        let pic = picture.clone();
        adw::StyleManager::default().connect_dark_notify(move |m| {
            backdrop::paint(&pic, m.is_dark());
        });
        let pic = picture.clone();
        adw::StyleManager::default().connect_high_contrast_notify(move |_| {
            pic.set_visible(style::glass_enabled());
        });
        picture.set_visible(style::glass_enabled());

        let app = self.ctx.app.clone();
        self.win.connect_close_request(move |_| {
            app.window.borrow_mut().take();
            glib::Propagation::Proceed
        });
    }

    /// Selects a page by id.
    pub fn select(&self, id: &str) {
        if let Some(i) = PAGES.iter().position(|(p, _, _)| *p == id)
            && let Some(row) = i32::try_from(i).ok().and_then(|i| self.sidebar.row_at_index(i))
        {
            self.sidebar.select_row(Some(&row));
        }
    }

    /// Raises the window.
    pub fn present(&self) {
        // The autostart entry may have been changed elsewhere (GNOME Tweaks, install script).
        self.prefs.sync();
        self.win.present();
    }

    /// Development aid: `MSI_CENTER_SCREENSHOT=out.png` (optionally `MSI_CENTER_PAGE=fans`) renders the
    /// window to a PNG after 3 s and quits. Lets visual changes be checked without a screenshot portal.
    pub fn schedule_screenshot(self: &Rc<Self>) {
        let Some(path) = std::env::var_os("MSI_CENTER_SCREENSHOT") else {
            return;
        };
        if let Ok(page) = std::env::var("MSI_CENTER_PAGE") {
            self.select(&page);
        }
        let w = Rc::downgrade(self);
        glib::timeout_add_local_once(std::time::Duration::from_secs(3), move || {
            let Some(me) = w.upgrade() else { return };
            let Some(clock) = me.win.frame_clock() else { return };
            // Snapshot right after a paint, when every allocation is current.
            let done = Rc::new(std::cell::Cell::new(false));
            let w = Rc::downgrade(&me);
            clock.connect_after_paint(move |_| {
                if done.replace(true) {
                    return;
                }
                if let Some(me) = w.upgrade() {
                    me.screenshot(&path);
                }
            });
            me.win.queue_draw();
        });
    }

    fn screenshot(&self, path: &std::ffi::OsStr) {
        let win = &self.win;
        let snap = gtk::Snapshot::new();
        if let Some(content) = win.content()
            && let Some(parent) = content.parent()
        {
            parent.snapshot_child(&content, &snap);
        }
        let renderer = win.native().and_then(|n| n.renderer());
        if let (Some(node), Some(r)) = (snap.to_node(), renderer) {
            let tex = r.render_texture(&node, None);
            if let Err(e) = tex.save_to_png(path) {
                tracing::warn!("screenshot: {e}");
            }
        } else {
            tracing::warn!("screenshot: nothing to render");
        }
        self.ctx.app.hold.borrow_mut().take();
        self.ctx.app.gtk.quit();
    }

    /// Whether the window has focus.
    pub fn is_active(&self) -> bool {
        self.win.is_active()
    }

    /// What pages need (toasts, app).
    pub fn ctx(&self) -> &Ctx {
        &self.ctx
    }

    /// Shows the selected profile in both switches.
    pub fn show_profile(&self, shown: Option<msi_core::profile::Profile>, pending: bool) {
        let enabled = {
            let s = self.ctx.app.shared.borrow();
            s.daemon_up == Some(true) && !s.status.as_ref().is_some_and(|st| st.read_only)
        };
        self.home.show_profile(shown, pending);
        self.header_profiles.set(shown, pending, enabled);
    }

    /// Shows the selected fan profile (Ventilateurs switch, Accueil chip).
    pub fn show_fan_profile(&self, shown: Option<msi_core::fan_profile::FanProfile>, pending: bool) {
        let enabled = {
            let s = self.ctx.app.shared.borrow();
            s.daemon_up == Some(true) && !s.status.as_ref().is_some_and(|st| st.read_only)
        };
        self.home.show_fan_profile(shown, pending);
        self.fans.show_fan_profile(shown, pending, enabled);
    }

    /// The dGPU stays on (Éco): inline on Accueil, banner elsewhere.
    pub fn show_gpu_busy(&self, text: &str) {
        self.home.show_gpu_busy(text);
        if self.stack.visible_child_name().as_deref() != Some("home") {
            self.gpu_banner.set_revealed(true);
        }
        self.perf.load_holders();
    }

    fn show_safety(&self, e: &SafetyEvent) {
        let prefix = match e.severity() {
            Severity::Warning => "⚠ ",
            Severity::Info => "ℹ ",
        };
        self.safety_banner.set_title(&format!("{prefix}{}", e.describe()));
        self.safety_banner.set_revealed(true);
        if e.severity() == Severity::Warning {
            let t = adw::Toast::new(&e.describe());
            t.set_use_markup(false);
            t.set_priority(adw::ToastPriority::High);
            self.ctx.toasts.add_toast(t);
        }
    }

    /// Replays the known state into a new window.
    pub fn replay(&self, s: &Shared) {
        match s.daemon_up {
            Some(true) => self.handle(&Event::DaemonUp {
                restore_factory: s.restore_factory,
            }),
            Some(false) => {
                if let Some(e) = &s.daemon_error {
                    self.handle(&Event::DaemonDown(e.clone()));
                }
            }
            None => {}
        }
        if let Some(st) = &s.status {
            self.handle(&Event::Daemon(DaemonEvent::Status(st.clone())));
        }
        if let Some(t) = &s.telemetry {
            self.handle(&Event::Daemon(DaemonEvent::Telemetry(t.clone())));
        }
        match s.clipd_up {
            Some(true) => self.handle(&Event::ClipdUp),
            Some(false) => {
                if let Some(e) = &s.clipd_error {
                    self.handle(&Event::ClipdDown(e.clone()));
                }
            }
            None => {}
        }
    }

    fn update_connection(&self) {
        let s = self.ctx.app.shared.borrow();
        let word = |up: Option<bool>| match up {
            Some(true) => "● connecté",
            Some(false) => "○ injoignable",
            None => "… connexion",
        };
        self.connection.set_label(&format!(
            "msi-daemon {}\nclipd {}",
            word(s.daemon_up),
            word(s.clipd_up)
        ));
    }

    /// Applies a backend event.
    pub fn handle(&self, ev: &Event) {
        match ev {
            Event::DaemonUp { restore_factory } => {
                self.daemon_banner.set_revealed(false);
                for g in [&self.home.gate, &self.perf.gate, &self.battery.gate] {
                    g.up();
                }
                self.fans.gate.up();
                self.fans.daemon_up(*restore_factory);
                self.perf.load_holders();
                self.update_connection();
            }
            Event::DaemonDown(e) => {
                self.daemon_banner.set_revealed(true);
                self.read_only_banner.set_revealed(false);
                for g in [
                    &self.home.gate,
                    &self.fans.gate,
                    &self.perf.gate,
                    &self.battery.gate,
                ] {
                    g.down(Some(e));
                }
                self.update_connection();
            }
            Event::Daemon(DaemonEvent::Status(s)) => {
                if s.read_only {
                    self.read_only_banner.set_title(&format!(
                        "Mode lecture seule : {} Les ventilateurs suivent la courbe d'usine.",
                        if s.read_only_reason.is_empty() {
                            "firmware EC non pris en charge."
                        } else {
                            s.read_only_reason.as_str()
                        }
                    ));
                }
                self.read_only_banner.set_revealed(s.read_only);
                if s.gpu_state != "active" {
                    self.gpu_banner.set_revealed(false);
                }
                self.home.set_status(s);
                self.fans.set_status(s);
                self.perf.set_status(s);
                self.battery.set_status(s);
            }
            Event::Daemon(DaemonEvent::Telemetry(t)) => {
                self.home.set_telemetry(t);
                self.fans.set_telemetry(t);
            }
            Event::Daemon(DaemonEvent::Safety(e)) if e.kind == "gpu_busy" => {
                self.show_gpu_busy(&e.describe());
            }
            Event::Daemon(DaemonEvent::Safety(e)) => self.show_safety(e),
            Event::ClipdUp => {
                self.clipboard.up();
                self.update_connection();
            }
            Event::ClipdDown(e) => {
                self.clipboard.gate.down(Some(e));
                self.update_connection();
            }
            Event::ClipsChanged => self.clipboard.changed(),
        }
    }
}
