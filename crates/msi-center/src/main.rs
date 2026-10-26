//! `msi-center`: GTK4/libadwaita front-end of msi-daemon (system bus) and clipd (session bus).
//!
//! `msi-center --background` (login autostart) keeps the application running without a window: it then
//! only turns safety events into desktop notifications. Activating it again (app menu, extension, D-Bus
//! activation) opens the window in the running instance.
#![forbid(unsafe_code)]

mod ui;

use std::rc::Rc;

use adw::prelude::*;
use gtk::{gio, glib};
use msi_center::APP_ID;
use msi_center::backend::Backend;
use msi_center::model;
use msi_core::fan_profile::FanProfile;
use msi_core::profile::Profile;
use ui::app::App;

fn install_actions(app: &Rc<App>) {
    let gtk_app = &app.gtk;
    let quit = gio::SimpleAction::new("quit", None);
    let a = app.clone();
    quit.connect_activate(move |_, _| {
        a.hold.borrow_mut().take();
        a.gtk.quit();
    });
    gtk_app.add_action(&quit);

    let activate = gio::SimpleAction::new("activate", None);
    let a = app.clone();
    activate.connect_activate(move |_, _| a.present());
    gtk_app.add_action(&activate);

    let about = gio::SimpleAction::new("about", None);
    let a = app.clone();
    about.connect_activate(move |_, _| {
        let dialog = adw::AboutDialog::builder()
            .application_name("MSI Center")
            .application_icon(APP_ID)
            .developer_name("Blazorde")
            .version(env!("CARGO_PKG_VERSION"))
            .comments(
                "Ventilateurs, profils, carte graphique, batterie et presse-papiers pour le \
                 MSI Stealth 14 AI Studio.",
            )
            .website(env!("CARGO_PKG_REPOSITORY"))
            .license_type(gtk::License::Gpl30)
            .build();
        let parent = a.gtk.active_window();
        dialog.present(parent.as_ref());
    });
    gtk_app.add_action(&about);

    // Profiles: `app.profile::eco` (stateful: its state is the shown profile) for menus, shortcuts and the
    // extension; `app.profile-eco` … without parameter for the desktop-file actions (dock right-click).
    let profile = gio::SimpleAction::new_stateful(
        model::PROFILE_ACTION,
        Some(glib::VariantTy::STRING),
        &"".to_variant(),
    );
    let a = app.clone();
    profile.connect_activate(move |_, v| {
        let target = v.and_then(glib::Variant::str);
        if let Some(p) = model::parse_profile_action(model::PROFILE_ACTION, target) {
            a.apply_profile(p);
        } else {
            tracing::warn!("app.profile: unknown profile {target:?}");
        }
    });
    // Activating with the current state (menus) must not be refused by GAction's state check.
    profile.connect_change_state(|_, _| {});
    gtk_app.add_action(&profile);
    for (i, p) in Profile::ALL.into_iter().enumerate() {
        let name = model::profile_action_name(p);
        let action = gio::SimpleAction::new(&name, None);
        let a = app.clone();
        action.connect_activate(move |_, _| a.apply_profile(p));
        gtk_app.add_action(&action);
        gtk_app.set_accels_for_action(
            &format!("app.{}::{}", model::PROFILE_ACTION, p.as_str()),
            &[&format!("<Control>{}", i + 1)],
        );
    }

    // Fan profiles: `app.fan::auto` (stateful) and `app.fan-auto` … without parameter.
    let fan = gio::SimpleAction::new_stateful(
        model::FAN_PROFILE_ACTION,
        Some(glib::VariantTy::STRING),
        &"".to_variant(),
    );
    let a = app.clone();
    fan.connect_activate(move |_, v| {
        let target = v.and_then(glib::Variant::str);
        if let Some(fp) = model::parse_fan_profile_action(model::FAN_PROFILE_ACTION, target) {
            a.apply_fan_profile(fp);
        } else {
            tracing::warn!("app.fan: unknown fan profile {target:?}");
        }
    });
    fan.connect_change_state(|_, _| {});
    gtk_app.add_action(&fan);
    for fp in FanProfile::ALL {
        let action = gio::SimpleAction::new(&model::fan_profile_action_name(fp), None);
        let a = app.clone();
        action.connect_activate(move |_, _| a.apply_fan_profile(fp));
        gtk_app.add_action(&action);
    }

    gtk_app.set_accels_for_action("app.quit", &["<Control>q"]);
    gtk_app.set_accels_for_action("window.close", &["<Control>w"]);
    for (i, page) in ["home", "fans", "perf", "battery", "clipboard", "prefs"]
        .iter()
        .enumerate()
    {
        gtk_app.set_accels_for_action(&format!("win.page::{page}"), &[&format!("<Alt>{}", i + 1)]);
    }
}

fn startup(app: &Rc<App>) {
    ui::style::install();
    ui::style::set_theme(app.settings.borrow().theme);
    #[cfg(debug_assertions)]
    if let Some(display) = gtk::gdk::Display::default() {
        // Development runs: use the icons of the source tree.
        gtk::IconTheme::for_display(&display)
            .add_search_path(concat!(env!("CARGO_MANIFEST_DIR"), "/data/icons"));
    }
    gtk::Window::set_default_icon_name(APP_ID);
    install_actions(app);
    if app.start_hidden.get() {
        *app.hold.borrow_mut() = Some(app.gtk.hold());
    }
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    if msi_center::demo::enabled() {
        tracing::info!("demo mode: simulated services, no bus is contacted");
        app.backend.start_demo(&tx);
    } else {
        app.backend.start(&tx);
    }
    let a = app.clone();
    glib::spawn_future_local(async move {
        while let Some(ev) = rx.recv().await {
            a.on_event(&ev);
        }
    });
}

/// Re-executes the binary with the variables of [`msi_center::gpu_env`] (no return on success).
fn keep_off_the_dgpu() {
    use std::os::unix::process::CommandExt as _;
    let vars = msi_center::gpu_env::wanted_now();
    if vars.is_empty() {
        return;
    }
    let Ok(exe) = std::env::current_exe() else {
        return;
    };
    let err = std::process::Command::new(exe)
        .args(std::env::args_os().skip(1))
        .envs(vars)
        .exec();
    eprintln!("msi-center: re-exec failed ({err}); the NVIDIA GPU may be woken up");
}

fn main() -> glib::ExitCode {
    keep_off_the_dgpu();
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("msi_center=info")),
        )
        .init();
    let backend = match Backend::new() {
        Ok(b) => b,
        Err(e) => {
            eprintln!("msi-center: cannot start the D-Bus runtime: {e}");
            return glib::ExitCode::FAILURE;
        }
    };
    let mut builder = adw::Application::builder().application_id(APP_ID);
    if msi_center::demo::enabled() {
        // Never hand over to (or take over) a real instance of the app.
        builder = builder.flags(gio::ApplicationFlags::NON_UNIQUE);
    }
    let gtk_app = builder.build();
    gtk_app.add_main_option(
        "background",
        glib::Char::from(0),
        glib::OptionFlags::NONE,
        glib::OptionArg::None,
        "Démarrer sans fenêtre (lancement à l'ouverture de session)",
        None,
    );
    let app = App::new(gtk_app.clone(), backend);

    let a = app.clone();
    gtk_app.connect_handle_local_options(move |g, dict| {
        if dict.contains("background") {
            a.start_hidden.set(true);
            if let Err(e) = g.register(None::<&gio::Cancellable>) {
                eprintln!("msi-center: {e}");
                return std::ops::ControlFlow::Break(glib::ExitCode::FAILURE);
            }
            if g.is_remote() {
                // Already running: nothing to do, and above all no window.
                return std::ops::ControlFlow::Break(glib::ExitCode::SUCCESS);
            }
        }
        std::ops::ControlFlow::Continue(())
    });
    let a = app.clone();
    gtk_app.connect_startup(move |_| startup(&a));
    let a = app.clone();
    gtk_app.connect_activate(move |_| {
        if a.start_hidden.replace(false) {
            tracing::info!("started in the background");
            return;
        }
        a.present();
    });
    gtk_app.run()
}
