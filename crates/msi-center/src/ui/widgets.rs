//! Small building blocks: glass cards, arc gauges, the "service unavailable" gate, the page context.

use std::cell::{Cell, RefCell};
use std::fmt::Write as _;
use std::future::Future;
use std::rc::Rc;

use adw::prelude::*;
use gtk::glib;
use msi_center::chart;
use msi_center::errors::{AppError, Service};
use msi_center::model::TempLevel;

use super::app::App;

/// What pages need from the window.
#[derive(Debug)]
pub struct Ctx {
    /// The application.
    pub app: Rc<App>,
    /// Toasts.
    pub toasts: adw::ToastOverlay,
}

impl Ctx {
    /// Shows a toast.
    pub fn toast(&self, msg: &str) {
        let t = adw::Toast::new(msg);
        t.set_use_markup(false);
        t.set_timeout(4);
        self.toasts.add_toast(t);
    }

    /// Shows an error; an unavailable service also triggers a reconnection.
    pub fn error(&self, e: &AppError) {
        tracing::warn!("{e}");
        if let AppError::Unavailable(service, _) = e {
            self.app.backend.retry(*service);
        }
        let t = adw::Toast::new(&e.message_fr());
        t.set_use_markup(false);
        t.set_timeout(if matches!(e, AppError::NotAuthorized) {
            6
        } else {
            5
        });
        self.toasts.add_toast(t);
    }

    /// Runs a future on the GTK main loop.
    pub fn spawn(fut: impl Future<Output = ()> + 'static) {
        glib::spawn_future_local(fut);
    }
}

/// A translucent glass card containing `child`.
pub fn glass(child: &impl IsA<gtk::Widget>) -> gtk::Box {
    let b = gtk::Box::new(gtk::Orientation::Vertical, 10);
    b.add_css_class("glass");
    b.append(child);
    b
}

/// Heading of a section, also exposed as an accessible heading.
pub fn section_title(text: &str) -> gtk::Label {
    gtk::Label::builder()
        .label(text)
        .xalign(0.0)
        .css_classes(["section-title"])
        .accessible_role(gtk::AccessibleRole::Heading)
        .margin_top(6)
        .build()
}

/// A dimmed, wrapping explanation.
pub fn hint(text: &str) -> gtk::Label {
    gtk::Label::builder()
        .label(text)
        .xalign(0.0)
        .wrap(true)
        .css_classes(["dim"])
        .build()
}

/// Vertical page container in a scrolled, width-clamped view.
pub fn page_scroller(content: &gtk::Box) -> gtk::ScrolledWindow {
    content.set_margin_top(12);
    content.set_margin_bottom(28);
    content.set_margin_start(16);
    content.set_margin_end(16);
    let clamp = adw::Clamp::builder()
        .maximum_size(1100)
        .tightening_threshold(800)
        .child(content)
        .build();
    gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .vexpand(true)
        .child(&clamp)
        .build()
}

/// Arc gauge card: title, arc, value text and an optional level word. The arc is decorative; the value
/// and level are text (accessible, and not conveyed by colour only).
#[derive(Debug)]
pub struct Gauge {
    /// Card root.
    pub root: gtk::Box,
    area: gtk::DrawingArea,
    value: gtk::Label,
    detail: gtk::Label,
    level: gtk::Label,
    title: String,
    fraction: Rc<Cell<f64>>,
    known: Rc<Cell<bool>>,
}

impl Gauge {
    /// A gauge titled `title` with an icon.
    pub fn new(title: &str, icon: &str) -> Self {
        let fraction = Rc::new(Cell::new(0.0));
        let known = Rc::new(Cell::new(false));
        let area = gtk::DrawingArea::builder()
            .content_width(92)
            .content_height(92)
            .accessible_role(gtk::AccessibleRole::Presentation)
            .build();
        {
            let fraction = fraction.clone();
            let known = known.clone();
            area.set_draw_func(move |a, cr, w, h| {
                draw_gauge(a, cr, f64::from(w), f64::from(h), fraction.get(), known.get());
            });
        }
        let value = gtk::Label::builder()
            .label(msi_center::format::UNKNOWN)
            .css_classes(["gauge-value"])
            .build();
        let overlay = gtk::Overlay::builder()
            .child(&area)
            .halign(gtk::Align::Center)
            .build();
        overlay.add_overlay(&value);
        let head = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        head.append(&gtk::Image::from_icon_name(icon));
        head.append(
            &gtk::Label::builder()
                .label(title)
                .css_classes(["card-title"])
                .xalign(0.0)
                .hexpand(true)
                .ellipsize(gtk::pango::EllipsizeMode::End)
                .build(),
        );
        let level = gtk::Label::builder()
            .css_classes(["level-badge"])
            .visible(false)
            .build();
        head.append(&level);
        let detail = gtk::Label::builder()
            .css_classes(["dim", "numeric", "caption"])
            .label(" ")
            .build();
        let root = gtk::Box::new(gtk::Orientation::Vertical, 6);
        root.add_css_class("glass");
        root.add_css_class("compact");
        root.add_css_class("interactive");
        root.set_size_request(168, -1);
        root.append(&head);
        root.append(&overlay);
        root.append(&detail);
        root.set_accessible_role(gtk::AccessibleRole::Group);
        root.update_property(&[gtk::accessible::Property::Label(title)]);
        Self {
            root,
            area,
            value,
            detail,
            level,
            title: title.to_owned(),
            fraction,
            known,
        }
    }

    /// Updates the gauge. `fraction` is `None` for an unknown value.
    pub fn set(&self, text: &str, fraction: Option<f64>, detail: &str, level: Option<TempLevel>) {
        self.value.set_label(text);
        self.detail
            .set_label(if detail.is_empty() { " " } else { detail });
        self.known.set(fraction.is_some());
        self.fraction.set(fraction.unwrap_or(0.0));
        for l in [TempLevel::Normal, TempLevel::Hot, TempLevel::Critical] {
            self.level.remove_css_class(l.css_class());
        }
        let mut desc = format!("{} : {text}", self.title);
        if let Some(l) = level {
            self.level.set_label(l.label());
            self.level.add_css_class(l.css_class());
            self.level.set_visible(true);
            let _ = write!(desc, " ({})", l.label());
        } else {
            self.level.set_visible(false);
        }
        if !detail.is_empty() {
            let _ = write!(desc, ", {detail}");
        }
        self.root
            .update_property(&[gtk::accessible::Property::Description(&desc)]);
        self.area.queue_draw();
    }
}

fn draw_gauge(area: &gtk::DrawingArea, cr: &gtk::cairo::Context, w: f64, h: f64, fraction: f64, known: bool) {
    let fg = area.color();
    let size = w.min(h);
    let (cx, cy) = (w / 2.0, h / 2.0);
    let line = (size * 0.09).max(5.0);
    let r = size / 2.0 - line / 2.0 - 1.0;
    cr.set_line_cap(gtk::cairo::LineCap::Round);
    cr.set_line_width(line);
    cr.set_source_rgba(
        f64::from(fg.red()),
        f64::from(fg.green()),
        f64::from(fg.blue()),
        0.12,
    );
    cr.arc(
        cx,
        cy,
        r,
        chart::GAUGE_START,
        chart::GAUGE_START + chart::GAUGE_SWEEP,
    );
    let _ = cr.stroke();
    if !known || fraction <= 0.0 {
        return;
    }
    let grad = gtk::cairo::LinearGradient::new(cx - r, cy + r, cx + r, cy - r);
    grad.add_color_stop_rgb(0.0, 0.949, 0.549, 0.220); // #F28C38
    grad.add_color_stop_rgb(0.6, 0.753, 0.212, 0.173); // #C0362C
    grad.add_color_stop_rgb(1.0, 0.55, 0.13, 0.12);
    let _ = cr.set_source(&grad);
    cr.arc(cx, cy, r, chart::GAUGE_START, chart::gauge_end(fraction));
    let _ = cr.stroke();
}

/// Switches a page between its content and a friendly "service unavailable" state with a retry button.
#[derive(Debug)]
pub struct Gate {
    /// Root widget.
    pub root: gtk::Stack,
    status: adw::StatusPage,
}

impl Gate {
    /// Wraps `content` for `service`.
    pub fn new(ctx: &Rc<Ctx>, service: Service, content: &impl IsA<gtk::Widget>) -> Self {
        let (title, icon) = match service {
            Service::Daemon => ("Service MSI indisponible", "dialog-warning-symbolic"),
            Service::Clipd => ("Presse-papiers indisponible", "edit-paste-symbolic"),
        };
        let retry = gtk::Button::builder()
            .label("Réessayer")
            .halign(gtk::Align::Center)
            .css_classes(["pill", "suggested-action"])
            .build();
        let c = ctx.clone();
        retry.connect_clicked(move |_| {
            c.app.backend.retry(service);
            c.toast("Nouvelle tentative de connexion…");
        });
        let status = adw::StatusPage::builder()
            .title(title)
            .icon_name(icon)
            .child(&retry)
            .build();
        let loading = adw::StatusPage::builder()
            .title("Connexion…")
            .child(&adw::Spinner::new())
            .build();
        let root = gtk::Stack::builder()
            .transition_type(gtk::StackTransitionType::Crossfade)
            .transition_duration(220)
            .vexpand(true)
            .build();
        root.add_named(content, Some("content"));
        root.add_named(&status, Some("down"));
        root.add_named(&loading, Some("loading"));
        root.set_visible_child_name("loading");
        Self { root, status }
    }

    /// Shows the content.
    pub fn up(&self) {
        self.root.set_visible_child_name("content");
    }

    /// Shows the unavailable state with `e`'s explanation.
    pub fn down(&self, e: Option<&AppError>) {
        let text = e.map_or_else(String::new, AppError::message_fr);
        // The description is Pango markup; service messages may contain `<` or `&`.
        self.status.set_description(Some(&format!(
            "{}\nNouvelle tentative automatique toutes les 10 secondes.",
            glib::markup_escape_text(&text)
        )));
        self.root.set_visible_child_name("down");
    }
}

/// A "debouncer": runs the latest scheduled closure after `ms` milliseconds of quiet.
#[derive(Debug, Default)]
pub struct Debounce {
    source: RefCell<Option<glib::SourceId>>,
}

impl Debounce {
    /// Schedules `f`, cancelling a pending one.
    pub fn run(self: &Rc<Self>, ms: u64, f: impl FnOnce() + 'static) {
        if let Some(id) = self.source.borrow_mut().take() {
            id.remove();
        }
        let me = Rc::downgrade(self);
        let id = glib::timeout_add_local_once(std::time::Duration::from_millis(ms), move || {
            if let Some(me) = me.upgrade() {
                me.source.borrow_mut().take();
            }
            f();
        });
        *self.source.borrow_mut() = Some(id);
    }
}
