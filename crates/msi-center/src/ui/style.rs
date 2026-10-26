//! Loads the Liquid Glass CSS, follows light/dark and turns it off in high contrast mode.

use gtk::gdk;
use msi_center::settings::Theme;

const LIGHT: &str = include_str!("../../data/style.css");
const DARK: &str = include_str!("../../data/style-dark.css");

/// Installs the providers on the default display and keeps them in sync with the style manager.
pub fn install() {
    let Some(display) = gdk::Display::default() else {
        tracing::warn!("no display: custom style not loaded");
        return;
    };
    let base = gtk::CssProvider::new();
    base.load_from_string(LIGHT);
    let dark = gtk::CssProvider::new();
    dark.load_from_string(DARK);
    let manager = adw::StyleManager::default();
    let apply = move |m: &adw::StyleManager| {
        gtk::style_context_remove_provider_for_display(&display, &base);
        gtk::style_context_remove_provider_for_display(&display, &dark);
        if m.is_high_contrast() {
            // Plain Adwaita high contrast: translucency would only hurt legibility.
            return;
        }
        gtk::style_context_add_provider_for_display(
            &display,
            &base,
            gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );
        if m.is_dark() {
            gtk::style_context_add_provider_for_display(
                &display,
                &dark,
                gtk::STYLE_PROVIDER_PRIORITY_APPLICATION + 1,
            );
        }
    };
    apply(&manager);
    let apply = std::rc::Rc::new(apply);
    let a = apply.clone();
    manager.connect_dark_notify(move |m| a(m));
    manager.connect_high_contrast_notify(move |m| apply(m));
}

/// Applies the theme preference.
pub fn set_theme(theme: Theme) {
    adw::StyleManager::default().set_color_scheme(match theme {
        Theme::System => adw::ColorScheme::Default,
        Theme::Light => adw::ColorScheme::ForceLight,
        Theme::Dark => adw::ColorScheme::ForceDark,
    });
}

/// Whether the current style is dark.
pub fn is_dark() -> bool {
    adw::StyleManager::default().is_dark()
}

/// Whether the glass style is active (not high contrast).
pub fn glass_enabled() -> bool {
    !adw::StyleManager::default().is_high_contrast()
}
