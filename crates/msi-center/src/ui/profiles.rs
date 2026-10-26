//! The profile switch: three glass segments (Éco / Équilibré / Gaming). One click applies.
//!
//! Two sizes share the logic: [`Variant::Hero`] on Accueil (icon, name, one-line description, state word) and
//! [`Variant::Compact`] in the header bar of the other pages (icons, tooltips). The active segment is marked
//! by a check and the word « Actif » as well as by the accent glow (never by colour alone); while a profile
//! is being applied its segment shows a spinner and « Application… ».

use std::cell::Cell;
use std::rc::Rc;

use adw::prelude::*;
use msi_center::model;
use msi_core::profile::Profile;

/// Size of the switch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Variant {
    /// Large segments with text.
    Hero,
    /// Icon-only segments for a header bar.
    Compact,
}

#[derive(Debug)]
struct Segment {
    profile: Profile,
    button: gtk::ToggleButton,
    /// Hero: state line (spinner / check + word). Compact: icon ↔ spinner.
    state: gtk::Stack,
    word: Option<gtk::Label>,
}

/// The switch.
#[derive(Debug)]
pub struct ProfileSwitch {
    /// Root widget.
    pub root: gtk::Box,
    segments: Vec<Segment>,
    syncing: Rc<Cell<bool>>,
}

fn hero_child(p: Profile) -> (gtk::Box, gtk::Stack, gtk::Label) {
    let glyph = gtk::Box::builder()
        .css_classes(["profile-glyph"])
        .valign(gtk::Align::Center)
        .halign(gtk::Align::Start)
        .build();
    let icon = gtk::Image::builder()
        .icon_name(model::profile_icon(p))
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
        .build();
    state.add_named(&gtk::Box::new(gtk::Orientation::Horizontal, 0), Some("idle"));
    state.add_named(&active, Some("active"));
    state.add_named(&busy, Some("busy"));

    let top = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    top.append(&glyph);
    top.append(&state);

    let b = gtk::Box::new(gtk::Orientation::Vertical, 4);
    b.append(&top);
    b.append(
        &gtk::Label::builder()
            .label(model::profile_label(p.as_str()))
            .xalign(0.0)
            .margin_top(8)
            .css_classes(["profile-name"])
            .build(),
    );
    b.append(
        &gtk::Label::builder()
            .label(model::profile_tagline(p))
            .xalign(0.0)
            .wrap(true)
            .natural_wrap_mode(gtk::NaturalWrapMode::Word)
            .css_classes(["profile-tagline"])
            .build(),
    );
    (b, state, word)
}

fn compact_child(p: Profile) -> gtk::Stack {
    let state = gtk::Stack::builder()
        .transition_type(gtk::StackTransitionType::Crossfade)
        .transition_duration(150)
        .build();
    state.add_named(&gtk::Image::from_icon_name(model::profile_icon(p)), Some("idle"));
    state.add_named(&adw::Spinner::new(), Some("busy"));
    state
}

impl ProfileSwitch {
    /// Builds the switch; `on_pick` runs when the user selects a profile (not on programmatic updates).
    pub fn new(variant: Variant, on_pick: impl Fn(Profile) + 'static) -> Self {
        let root = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .homogeneous(true)
            .css_classes(["profile-picker"])
            .build();
        root.set_accessible_role(gtk::AccessibleRole::Group);
        root.update_property(&[gtk::accessible::Property::Label("Profil")]);
        let on_pick = Rc::new(on_pick);
        let syncing = Rc::new(Cell::new(false));
        let mut segments: Vec<Segment> = Vec::new();
        for p in Profile::ALL {
            let label = model::profile_label(p.as_str());
            let tip = format!("{label} — {}", model::profile_tagline(p));
            let button = gtk::ToggleButton::builder()
                .tooltip_text(&tip)
                .css_classes(["profile-segment"])
                .build();
            button.update_property(&[
                gtk::accessible::Property::Label(label),
                gtk::accessible::Property::Description(&model::profile_description(p)),
            ]);
            let (state, word) = match variant {
                Variant::Hero => {
                    let (child, state, word) = hero_child(p);
                    button.set_child(Some(&child));
                    (state, Some(word))
                }
                Variant::Compact => {
                    let state = compact_child(p);
                    button.set_child(Some(&state));
                    (state, None)
                }
            };
            if let Some(first) = segments.first() {
                button.set_group(Some(&first.button));
            }
            let pick = on_pick.clone();
            let sync = syncing.clone();
            button.connect_toggled(move |b| {
                if b.is_active() && !sync.get() {
                    pick(p);
                }
            });
            root.append(&button);
            segments.push(Segment {
                profile: p,
                button,
                state,
                word,
            });
        }
        match variant {
            Variant::Hero => {
                root.add_css_class("hero");
                root.set_spacing(10);
            }
            Variant::Compact => {
                root.add_css_class("compact");
                root.set_spacing(2);
            }
        }
        Self {
            root,
            segments,
            syncing,
        }
    }

    /// Shows `shown` as selected; `pending` while it is being applied. `enabled` is `false` in read-only
    /// mode or without the daemon.
    pub fn set(&self, shown: Option<Profile>, pending: bool, enabled: bool) {
        self.syncing.set(true);
        for s in &self.segments {
            let on = shown == Some(s.profile);
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
}
