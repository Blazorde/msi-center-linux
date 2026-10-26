//! Glass segmented switches. One click applies.
//!
//! [`ProfileSwitch`]: three segments (Éco / Équilibré / Gaming) in two sizes: [`Variant::Hero`] on Accueil
//! (icon, name, one-line description, state word) and [`Variant::Compact`] in the header bar of the other
//! pages (icons, tooltips). [`FanProfileSwitch`]: the five fan profiles as hero tiles that wrap
//! ([`Variant::Tiles`]) on Ventilateurs, with a « Défaut » badge on the profile's default.
//!
//! The active segment is marked by a check and the word « Actif » as well as by the accent glow (never by
//! colour alone); while a choice is being applied its segment shows a spinner and « Application… ».

use std::cell::Cell;
use std::rc::Rc;

use adw::prelude::*;
use msi_center::model;
use msi_core::fan_profile::FanProfile;
use msi_core::profile::Profile;

/// Size of the switch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Variant {
    /// Large segments with text, in one row.
    Hero,
    /// Large segments with text that wrap on narrow windows.
    Tiles,
    /// Icon-only segments for a header bar.
    Compact,
}

/// What a switch can choose.
pub trait SwitchItem: Copy + PartialEq + std::fmt::Debug + 'static {
    /// Every choice, in display order.
    fn all() -> Vec<Self>;
    /// French name.
    fn label(self) -> &'static str;
    /// One short line under the name.
    fn tagline(self) -> &'static str;
    /// Symbolic icon.
    fn icon(self) -> &'static str;
    /// Accessible description.
    fn description(self) -> String;
}

impl SwitchItem for Profile {
    fn all() -> Vec<Self> {
        Self::ALL.to_vec()
    }
    fn label(self) -> &'static str {
        model::profile_label(self.as_str())
    }
    fn tagline(self) -> &'static str {
        model::profile_tagline(self)
    }
    fn icon(self) -> &'static str {
        model::profile_icon(self)
    }
    fn description(self) -> String {
        model::profile_description(self)
    }
}

impl SwitchItem for FanProfile {
    fn all() -> Vec<Self> {
        Self::ALL.to_vec()
    }
    fn label(self) -> &'static str {
        Self::label(self)
    }
    fn tagline(self) -> &'static str {
        Self::description(self)
    }
    fn icon(self) -> &'static str {
        model::fan_profile_icon(self)
    }
    fn description(self) -> String {
        Self::description(self).to_owned()
    }
}

#[derive(Debug)]
struct Segment<T> {
    item: T,
    button: gtk::ToggleButton,
    /// Hero: state line (spinner / check + word). Compact: icon ↔ spinner.
    state: gtk::Stack,
    word: Option<gtk::Label>,
    badge: Option<gtk::Label>,
}

/// A glass switch over `T`.
#[derive(Debug)]
pub struct GlassSwitch<T> {
    /// Root widget.
    pub root: gtk::Widget,
    segments: Vec<Segment<T>>,
    syncing: Rc<Cell<bool>>,
}

/// The performance profile switch.
pub type ProfileSwitch = GlassSwitch<Profile>;
/// The fan profile switch.
pub type FanProfileSwitch = GlassSwitch<FanProfile>;

fn hero_child<T: SwitchItem>(item: T, tiles: bool) -> (gtk::Box, gtk::Stack, gtk::Label, gtk::Label) {
    let glyph = gtk::Box::builder()
        .css_classes(["profile-glyph"])
        .valign(gtk::Align::Center)
        .halign(gtk::Align::Start)
        .build();
    let icon = gtk::Image::builder()
        .icon_name(item.icon())
        .pixel_size(20)
        .hexpand(true)
        .halign(gtk::Align::Center)
        .valign(gtk::Align::Center)
        .build();
    glyph.append(&icon);

    let word = gtk::Label::builder().css_classes(["profile-state-word"]).build();
    let check = gtk::Image::from_icon_name("object-select-symbolic");
    let active = gtk::Box::new(gtk::Orientation::Horizontal, 4);
    active.append(&check);
    active.append(&word);
    let spinner = adw::Spinner::builder()
        .width_request(14)
        .height_request(14)
        .build();
    let busy = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    busy.append(&spinner);
    busy.append(&gtk::Label::builder().label("Application…").build());
    let state = gtk::Stack::builder()
        .transition_type(gtk::StackTransitionType::Crossfade)
        .transition_duration(150)
        .halign(gtk::Align::End)
        .valign(gtk::Align::Center)
        .hexpand(true)
        .css_classes(["profile-state"])
        // Tiles: size to the visible page so « Application… » does not widen every tile.
        .hhomogeneous(!tiles)
        .interpolate_size(tiles)
        .build();
    // Idle: the « Défaut » badge of the profile's default choice (tiles), else nothing.
    let badge = gtk::Label::builder()
        .label("Défaut")
        .visible(false)
        .valign(gtk::Align::Center)
        .css_classes(["default-badge"])
        .build();
    let idle = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    idle.append(&badge);
    state.add_named(&idle, Some("idle"));
    state.add_named(&active, Some("active"));
    state.add_named(&busy, Some("busy"));

    let top = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    top.append(&glyph);
    top.append(&state);

    let b = gtk::Box::new(gtk::Orientation::Vertical, 4);
    b.append(&top);
    b.append(
        &gtk::Label::builder()
            .label(item.label())
            .xalign(0.0)
            .margin_top(8)
            .css_classes(["profile-name"])
            .build(),
    );
    b.append(
        &gtk::Label::builder()
            .label(item.tagline())
            .xalign(0.0)
            .wrap(true)
            .natural_wrap_mode(gtk::NaturalWrapMode::Word)
            // Tiles: a narrow natural width lets the flow box put the five tiles on one line.
            .max_width_chars(if tiles { 11 } else { -1 })
            .css_classes(["profile-tagline"])
            .build(),
    );
    (b, state, word, badge)
}

fn compact_child<T: SwitchItem>(item: T) -> gtk::Stack {
    let state = gtk::Stack::builder()
        .transition_type(gtk::StackTransitionType::Crossfade)
        .transition_duration(150)
        .build();
    state.add_named(&gtk::Image::from_icon_name(item.icon()), Some("idle"));
    state.add_named(&adw::Spinner::new(), Some("busy"));
    state
}

impl<T: SwitchItem> GlassSwitch<T> {
    /// Builds the switch; `name` is its accessible label; `on_pick` runs when the user selects an item (not on
    /// programmatic updates).
    pub fn new(variant: Variant, name: &str, on_pick: impl Fn(T) + 'static) -> Self {
        let on_pick = Rc::new(on_pick);
        let syncing = Rc::new(Cell::new(false));
        let mut segments: Vec<Segment<T>> = Vec::new();
        for item in T::all() {
            let label = item.label();
            let tip = format!("{label} — {}", item.tagline());
            let button = gtk::ToggleButton::builder()
                .tooltip_text(&tip)
                .css_classes(["profile-segment"])
                .build();
            button.update_property(&[
                gtk::accessible::Property::Label(label),
                gtk::accessible::Property::Description(&item.description()),
            ]);
            let (state, word, badge) = match variant {
                Variant::Hero | Variant::Tiles => {
                    let (child, state, word, badge) = hero_child(item, variant == Variant::Tiles);
                    button.set_child(Some(&child));
                    (state, Some(word), Some(badge))
                }
                Variant::Compact => {
                    let state = compact_child(item);
                    button.set_child(Some(&state));
                    (state, None, None)
                }
            };
            if let Some(first) = segments.first() {
                button.set_group(Some(&first.button));
            }
            let pick = on_pick.clone();
            let sync = syncing.clone();
            button.connect_toggled(move |b| {
                if b.is_active() && !sync.get() {
                    pick(item);
                }
            });
            segments.push(Segment {
                item,
                button,
                state,
                word,
                badge,
            });
        }
        let root: gtk::Widget = match variant {
            Variant::Hero | Variant::Compact => {
                let b = gtk::Box::builder()
                    .orientation(gtk::Orientation::Horizontal)
                    .homogeneous(true)
                    .css_classes(["profile-picker"])
                    .build();
                if variant == Variant::Hero {
                    b.add_css_class("hero");
                    b.set_spacing(10);
                } else {
                    b.add_css_class("compact");
                    b.set_spacing(2);
                }
                for s in &segments {
                    b.append(&s.button);
                }
                b.upcast()
            }
            Variant::Tiles => {
                let f = gtk::FlowBox::builder()
                    .selection_mode(gtk::SelectionMode::None)
                    .homogeneous(true)
                    .min_children_per_line(2)
                    .max_children_per_line(5)
                    .column_spacing(8)
                    .row_spacing(8)
                    .css_classes(["profile-picker", "hero", "tiles"])
                    .build();
                for s in &segments {
                    f.append(&s.button);
                    // Tab goes to the toggles, not to the flow box cells.
                    if let Some(cell) = s.button.parent() {
                        cell.set_focusable(false);
                    }
                }
                f.upcast()
            }
        };
        root.set_accessible_role(gtk::AccessibleRole::Group);
        root.update_property(&[gtk::accessible::Property::Label(name)]);
        Self {
            root,
            segments,
            syncing,
        }
    }

    /// Shows `shown` as selected; `pending` while it is being applied. `enabled` is `false` in read-only
    /// mode or without the daemon.
    pub fn set(&self, shown: Option<T>, pending: bool, enabled: bool) {
        self.syncing.set(true);
        for s in &self.segments {
            let on = shown == Some(s.item);
            s.button.set_active(on);
            s.button.set_sensitive(enabled);
            let busy = on && pending;
            if busy {
                s.button.add_css_class("pending");
            } else {
                s.button.remove_css_class("pending");
            }
            let page = match (busy, on, s.word.is_some()) {
                (true, ..) => "busy",
                (false, true, true) => "active",
                _ => "idle",
            };
            s.state.set_visible_child_name(page);
            if let Some(w) = &s.word {
                w.set_label(if on { "Actif" } else { "" });
            }
        }
        self.syncing.set(false);
    }

    /// Marks `default` with a « Défaut » badge while it is not the active choice (hero and tiles only; the
    /// switch's caption names the default too).
    pub fn set_default(&self, default: Option<T>) {
        for s in &self.segments {
            if let Some(b) = &s.badge {
                b.set_visible(default == Some(s.item));
            }
        }
    }
}
