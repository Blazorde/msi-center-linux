//! Application state shared by the window and the background mode.

use std::cell::{Cell, RefCell};
use std::path::PathBuf;
use std::rc::Rc;

use adw::prelude::*;
use gtk::{gio, glib};
use msi_center::backend::{Backend, Event};
use msi_center::dbus::DaemonEvent;
use msi_center::errors::AppError;
use msi_center::model::{DaemonStatus, SafetyEvent, Severity, Telemetry};
use msi_center::settings::{self, Settings};
use msi_core::profile::Profile;

use super::window::Window;

/// Last known state of both services, replayed to a window when it opens.
#[derive(Debug, Default)]
pub struct Shared {
    /// `Some(true)` once the daemon answered, `Some(false)` when it is down, `None` before the first try.
    pub daemon_up: Option<bool>,
    /// Why the daemon is down.
    pub daemon_error: Option<AppError>,
    /// The daemon has `RestoreFactory`.
    pub restore_factory: bool,
    /// Last status.
    pub status: Option<DaemonStatus>,
    /// Last telemetry sample.
    pub telemetry: Option<Telemetry>,
    /// clipd state, as for the daemon.
    pub clipd_up: Option<bool>,
    /// Why clipd is down.
    pub clipd_error: Option<AppError>,
}

/// The application.
#[derive(Debug)]
pub struct App {
    /// GTK application.
    pub gtk: adw::Application,
    /// D-Bus bridge.
    pub backend: Backend,
    /// Service state.
    pub shared: RefCell<Shared>,
    /// Open window, if any.
    pub window: RefCell<Option<Rc<Window>>>,
    /// Keeps the process alive without a window (`--background`).
    pub hold: RefCell<Option<gio::ApplicationHoldGuard>>,
    /// The first `activate` must not open a window (`--background`).
    pub start_hidden: Cell<bool>,
    /// Preferences.
    pub settings: RefCell<Settings>,
    /// `$XDG_CONFIG_HOME`.
    pub config_dir: PathBuf,
    /// Profile being applied (optimistic UI), if any.
    pub pending_profile: Cell<Option<Profile>>,
    /// Profile asked for before the daemon answered (action activated at startup), applied on `DaemonUp`.
    queued_profile: Cell<Option<Profile>>,
    /// Keeps the app alive while a profile asked without a window is being applied.
    profile_hold: RefCell<Option<gio::ApplicationHoldGuard>>,
}

impl App {
    /// Builds the state (no window yet).
    pub fn new(gtk: adw::Application, backend: Backend) -> Rc<Self> {
        let config_dir = glib::user_config_dir();
        let settings = settings::load(&settings::path_in(&config_dir));
        Rc::new(Self {
            gtk,
            backend,
            shared: RefCell::default(),
            window: RefCell::new(None),
            hold: RefCell::new(None),
            start_hidden: Cell::new(false),
            settings: RefCell::new(settings),
            config_dir,
            pending_profile: Cell::new(None),
            queued_profile: Cell::new(None),
            profile_hold: RefCell::new(None),
        })
    }

    /// The profile the UI should show as selected: the one being applied, else the daemon's.
    pub fn shown_profile(&self) -> Option<Profile> {
        self.pending_profile.get().or_else(|| {
            self.shared
                .borrow()
                .status
                .as_ref()
                .and_then(|s| s.profile.parse().ok())
        })
    }

    /// Applies `p` (actions, extension, both switches). Optimistic: the switches show `p` with a progress
    /// indicator at once; on failure they snap back and a toast (or a notification without a window)
    /// explains. Before the daemon answered, the request is queued and applied on connection.
    pub fn apply_profile(self: &Rc<Self>, p: Profile) {
        if self.profile_hold.borrow().is_none() && self.window.borrow().is_none() {
            // No window (desktop action, extension): stay alive until the call returns.
            *self.profile_hold.borrow_mut() = Some(self.gtk.hold());
        }
        if self.shared.borrow().daemon_up != Some(true) {
            self.queued_profile.set(Some(p));
            self.pending_profile.set(Some(p));
            self.refresh_profile_ui();
            return;
        }
        let current = self.shared.borrow().status.as_ref().map(|s| s.profile.clone());
        if current.as_deref() == Some(p.as_str()) && self.pending_profile.get().is_none() {
            self.profile_hold.borrow_mut().take();
            return;
        }
        self.pending_profile.set(Some(p));
        self.refresh_profile_ui();
        let me = self.clone();
        glib::spawn_future_local(async move {
            let res = me
                .backend
                .daemon(move |d| async move { d.set_profile(p.as_str()).await })
                .await;
            // A later click superseded this one: its own reply settles the UI.
            if me.pending_profile.get() != Some(p) {
                return;
            }
            me.pending_profile.set(None);
            let label = msi_center::model::profile_label(p.as_str());
            match res {
                Ok(()) => me.report(&format!("Profil {label} appliqué."), None),
                Err(e @ AppError::GpuBusy(_)) => {
                    // The EC part is applied: keep the profile, explain inline.
                    me.report(&format!("Profil {label} appliqué."), None);
                    if let Some(w) = me.window.borrow().as_ref() {
                        w.show_gpu_busy(&e.message_fr());
                    }
                }
                Err(e) => me.report(&e.message_fr(), Some(&e)),
            }
            me.refresh_profile_ui();
            me.profile_hold.borrow_mut().take();
        });
    }

    /// Toast in the window, or a notification without one.
    fn report(&self, msg: &str, err: Option<&AppError>) {
        if let Some(w) = self.window.borrow().as_ref() {
            match err {
                Some(e) => w.ctx().error(e),
                None => w.ctx().toast(msg),
            }
            return;
        }
        if let Some(e) = err {
            tracing::warn!("{e}");
        }
        let n = gio::Notification::new("MSI Center");
        n.set_body(Some(msg));
        n.set_default_action("app.activate");
        self.gtk.send_notification(Some("profile"), &n);
    }

    /// Pushes the shown profile to the window and the `app.profile` action state.
    pub fn refresh_profile_ui(&self) {
        let shown = self.shown_profile();
        if let Some(a) = self
            .gtk
            .lookup_action(msi_center::model::PROFILE_ACTION)
            .and_then(|a| a.downcast::<gio::SimpleAction>().ok())
        {
            a.set_state(&shown.map_or("", Profile::as_str).to_variant());
        }
        if let Some(w) = self.window.borrow().as_ref() {
            w.show_profile(shown, self.pending_profile.get().is_some());
        }
    }

    /// Saves the preferences (logged on failure).
    pub fn save_settings(&self) -> Result<(), String> {
        let s = *self.settings.borrow();
        settings::save(&settings::path_in(&self.config_dir), &s).map_err(|e| {
            tracing::warn!("saving settings: {e}");
            format!("Impossible d'enregistrer les réglages : {e}")
        })
    }

    /// Opens (or raises) the window.
    pub fn present(self: &Rc<Self>) {
        let existing = self.window.borrow().clone();
        let win = existing.unwrap_or_else(|| {
            let w = Window::new(self);
            *self.window.borrow_mut() = Some(w.clone());
            w.replay(&self.shared.borrow());
            w.schedule_screenshot();
            w
        });
        win.present();
    }

    /// Handles a backend event: updates the shared state, forwards to the window, notifies in background.
    pub fn on_event(self: &Rc<Self>, ev: &Event) {
        {
            let mut s = self.shared.borrow_mut();
            match ev {
                Event::DaemonUp { restore_factory } => {
                    s.daemon_up = Some(true);
                    s.daemon_error = None;
                    s.restore_factory = *restore_factory;
                }
                Event::DaemonDown(e) => {
                    s.daemon_up = Some(false);
                    s.daemon_error = Some(e.clone());
                    s.status = None;
                    s.telemetry = None;
                }
                Event::Daemon(DaemonEvent::Status(st)) => s.status = Some(st.clone()),
                Event::Daemon(DaemonEvent::Telemetry(t)) => s.telemetry = Some(t.clone()),
                Event::Daemon(DaemonEvent::Safety(_)) | Event::ClipsChanged => {}
                Event::ClipdUp => {
                    s.clipd_up = Some(true);
                    s.clipd_error = None;
                }
                Event::ClipdDown(e) => {
                    s.clipd_up = Some(false);
                    s.clipd_error = Some(e.clone());
                }
            }
        }
        let win = self.window.borrow().clone();
        let visible = win.as_ref().is_some_and(|w| w.is_active());
        if let Some(w) = win {
            w.handle(ev);
        }
        match ev {
            Event::Daemon(DaemonEvent::Safety(e)) if !visible => self.notify_safety(e),
            Event::Daemon(DaemonEvent::Status(_)) => self.refresh_profile_ui(),
            Event::DaemonUp { .. } => {
                if let Some(p) = self.queued_profile.take() {
                    self.pending_profile.set(None);
                    self.apply_profile(p);
                }
            }
            // A request queued before the daemon answered: the daemon is down, give up and say so.
            Event::DaemonDown(e) if self.queued_profile.take().is_some() => {
                self.pending_profile.set(None);
                self.report(&e.message_fr(), Some(e));
                self.refresh_profile_ui();
                self.profile_hold.borrow_mut().take();
            }
            _ => {}
        }
    }

    fn notify_safety(&self, e: &SafetyEvent) {
        let title = match e.severity() {
            Severity::Warning => "MSI Center : sécurité thermique",
            Severity::Info => "MSI Center",
        };
        let n = gio::Notification::new(title);
        n.set_body(Some(&e.describe()));
        n.set_default_action("app.activate");
        if e.severity() == Severity::Warning {
            n.set_priority(gio::NotificationPriority::High);
        }
        self.gtk.send_notification(Some("safety"), &n);
    }
}
