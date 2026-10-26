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
        })
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
        if let Event::Daemon(DaemonEvent::Safety(e)) = ev
            && !visible
        {
            self.notify_safety(e);
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
