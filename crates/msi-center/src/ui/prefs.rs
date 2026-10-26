//! Réglages: start at login, start in the background, theme.

use std::cell::Cell;
use std::rc::Rc;

use adw::prelude::*;
use msi_center::autostart;
use msi_center::settings::Theme;

use super::style;
use super::widgets::{Ctx, hint, page_scroller};

/// The page.
#[derive(Debug)]
pub struct Prefs {
    /// Root widget.
    pub root: gtk::ScrolledWindow,
    ctx: Rc<Ctx>,
    login: adw::SwitchRow,
    background: adw::SwitchRow,
    theme: adw::ToggleGroup,
    syncing: Cell<bool>,
}

fn exec_path() -> String {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.to_str().map(str::to_owned))
        .unwrap_or_else(|| "msi-center".to_owned())
}

impl Prefs {
    /// Builds the page.
    pub fn new(ctx: &Rc<Ctx>) -> Rc<Self> {
        let content = gtk::Box::new(gtk::Orientation::Vertical, 18);
        let start = adw::PreferencesGroup::builder()
            .title("Démarrage")
            .description(
                "MSI Center se lance à chaque ouverture de session. En arrière-plan, il ne montre pas de \
                 fenêtre mais vous prévient des alertes de sécurité thermique ; ouvrez-le depuis \
                 l'extension (« Ouvrir MSI Center ») ou le menu des applications.",
            )
            .build();
        let login = adw::SwitchRow::builder()
            .title("Lancer à l'ouverture de session")
            .subtitle("~/.config/autostart/org.blazorde.MsiCenter.desktop")
            .build();
        let background = adw::SwitchRow::builder()
            .title("Démarrer en arrière-plan")
            .subtitle("Sans fenêtre au démarrage")
            .build();
        start.add(&login);
        start.add(&background);
        content.append(&start);

        let look = adw::PreferencesGroup::builder().title("Apparence").build();
        let theme = adw::ToggleGroup::builder()
            .valign(gtk::Align::Center)
            .homogeneous(true)
            .build();
        for t in Theme::ALL {
            theme.add(adw::Toggle::builder().name(t.as_str()).label(t.label()).build());
        }
        let theme_row = adw::ActionRow::builder().title("Thème").build();
        theme_row.add_suffix(&theme);
        look.add(&theme_row);
        content.append(&look);
        content.append(&hint(
            "Style « Liquid Glass » : GTK ne peut pas flouter le bureau derrière la fenêtre (Wayland ne \
             le permet pas), le verre dépoli est donc simulé par un arrière-plan interne flouté. En mode \
             contraste élevé, le style transparent est désactivé.",
        ));

        let page = Rc::new(Self {
            root: page_scroller(&content),
            ctx: ctx.clone(),
            login,
            background,
            theme,
            syncing: Cell::new(false),
        });
        page.sync();
        let w = Rc::downgrade(&page);
        page.login.connect_active_notify(move |_| {
            if let Some(p) = w.upgrade()
                && !p.syncing.get()
            {
                p.write_autostart();
            }
        });
        let w = Rc::downgrade(&page);
        page.background.connect_active_notify(move |s| {
            if let Some(p) = w.upgrade()
                && !p.syncing.get()
            {
                p.ctx.app.settings.borrow_mut().start_minimised = s.is_active();
                if let Err(e) = p.ctx.app.save_settings() {
                    p.ctx.toast(&e);
                }
                p.write_autostart();
            }
        });
        let w = Rc::downgrade(&page);
        page.theme.connect_active_name_notify(move |g| {
            let (Some(p), Some(name)) = (w.upgrade(), g.active_name()) else {
                return;
            };
            if p.syncing.get() {
                return;
            }
            let t = Theme::parse(&name).unwrap_or_default();
            p.ctx.app.settings.borrow_mut().theme = t;
            style::set_theme(t);
            if let Err(e) = p.ctx.app.save_settings() {
                p.ctx.toast(&e);
            }
        });
        page
    }

    fn path(&self) -> std::path::PathBuf {
        autostart::path_in(&self.ctx.app.config_dir)
    }

    /// Reads the autostart entry and the settings into the widgets.
    pub fn sync(&self) {
        let settings = *self.ctx.app.settings.borrow();
        let state = autostart::read(&self.path()).unwrap_or(autostart::State {
            enabled: false,
            background: settings.start_minimised,
        });
        self.syncing.set(true);
        self.login.set_active(state.enabled);
        self.background.set_active(if state.enabled {
            state.background
        } else {
            settings.start_minimised
        });
        self.theme.set_active_name(Some(settings.theme.as_str()));
        self.syncing.set(false);
    }

    fn write_autostart(&self) {
        let path = self.path();
        let res = if self.login.is_active() {
            autostart::enable(&path, &exec_path(), self.background.is_active())
        } else {
            autostart::disable(&path)
        };
        match res {
            Ok(()) if self.login.is_active() => {
                self.ctx.toast("MSI Center se lancera à l'ouverture de session.");
            }
            Ok(()) => self.ctx.toast("Lancement automatique désactivé."),
            Err(e) => {
                self.ctx
                    .toast(&format!("Impossible de modifier le démarrage automatique : {e}"));
                self.sync();
            }
        }
    }
}
