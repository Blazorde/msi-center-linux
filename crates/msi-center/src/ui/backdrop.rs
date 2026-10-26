//! The window backdrop: sunset gradient, blurred colour blobs, a faint Golden Gate silhouette and grain.
//!
//! GTK cannot blur what is behind the window on Wayland, so the "frosted glass" cards are translucent over
//! this in-app layer instead. It is drawn once into a [`gtk::Snapshot`] (with `push_blur`), rendered to a
//! texture by the window's renderer and shown in a [`gtk::Picture`]: no per-frame cost, no subclassing (the
//! crate forbids `unsafe`, which glib subclassing macros expand to).

use gtk::prelude::*;
use gtk::{gdk, glib, graphene, gsk};
use msi_center::chart;

/// Logical size of the rendered backdrop; the picture covers the window, so it is scaled.
const W: f32 = 1600.0;
const H: f32 = 1000.0;
const NOISE: usize = 128;

fn rgba(hex: u32, a: f32) -> gdk::RGBA {
    #[allow(clippy::cast_precision_loss, reason = "8-bit channels")]
    let c = |shift: u32| ((hex >> shift) & 0xff) as f32 / 255.0;
    gdk::RGBA::new(c(16), c(8), c(0), a)
}

fn blob(s: &gtk::Snapshot, x: f32, y: f32, r: f32, color: u32, alpha: f32) {
    let rect = graphene::Rect::new(x - r, y - r, 2.0 * r, 2.0 * r);
    s.append_radial_gradient(
        &rect,
        &graphene::Point::new(x, y),
        r,
        r,
        0.0,
        1.0,
        &[
            gsk::ColorStop::new(0.0, rgba(color, alpha)),
            gsk::ColorStop::new(0.55, rgba(color, alpha * 0.45)),
            gsk::ColorStop::new(1.0, rgba(color, 0.0)),
        ],
    );
}

fn noise_texture() -> gdk::Texture {
    let bytes = glib::Bytes::from_owned(chart::noise_rgba(NOISE, 0x00c0_362c));
    #[allow(clippy::cast_possible_truncation, clippy::cast_possible_wrap, reason = "128")]
    let size = NOISE as i32;
    gdk::MemoryTexture::new(size, size, gdk::MemoryFormat::R8g8b8a8, &bytes, NOISE * 4).upcast()
}

#[allow(clippy::many_single_char_names, reason = "geometry")]
fn draw_bridge(s: &gtk::Snapshot, dark: bool) {
    let bounds = graphene::Rect::new(0.0, 0.0, W, H);
    let cr = s.append_cairo(&bounds);
    let b = chart::bridge(f64::from(W), f64::from(H));
    // International Orange, very faint: decoration only.
    let (r, g, bl) = (0.753, 0.212, 0.173);
    let a = if dark { 0.24 } else { 0.16 };
    cr.set_source_rgba(r, g, bl, a);
    // Art-deco towers: two legs joined by crossbars.
    for &(x, y, w, h) in &b.towers {
        let leg = w * 0.42;
        cr.rectangle(x - w * 0.6, y, leg, h);
        cr.rectangle(x + w * 1.6 - leg, y, leg, h);
        for k in [0.04, 0.22, 0.42, 0.62] {
            cr.rectangle(x - w * 0.6, y + h * k, w * 2.2, h * 0.035);
        }
    }
    let _ = cr.fill();
    cr.set_line_width(3.0);
    let (x0, y, x1) = b.deck;
    cr.move_to(x0, y);
    cr.line_to(x1, y);
    let _ = cr.stroke();
    cr.set_line_width(3.5);
    for (i, &(x, y)) in b.cable.iter().enumerate() {
        if i == 0 {
            cr.move_to(x, y);
        } else {
            cr.line_to(x, y);
        }
    }
    let _ = cr.stroke();
    cr.set_line_width(1.0);
    cr.set_source_rgba(r, g, bl, a * 0.7);
    for &(x, top, bottom) in &b.hangers {
        cr.move_to(x, top);
        cr.line_to(x, bottom);
    }
    let _ = cr.stroke();
}

/// Builds the render node of the backdrop.
pub fn node(dark: bool) -> Option<gsk::RenderNode> {
    let s = gtk::Snapshot::new();
    let bounds = graphene::Rect::new(0.0, 0.0, W, H);
    let (top, mid, bottom) = if dark {
        (0x0016_1724, 0x001f_2033, 0x002b_2d42)
    } else {
        (0x00ff_f7f1, 0x00fb_ece3, 0x00f3_dfd6)
    };
    s.append_linear_gradient(
        &bounds,
        &graphene::Point::new(0.0, 0.0),
        &graphene::Point::new(W * 0.35, H),
        &[
            gsk::ColorStop::new(0.0, rgba(top, 1.0)),
            gsk::ColorStop::new(0.55, rgba(mid, 1.0)),
            gsk::ColorStop::new(1.0, rgba(bottom, 1.0)),
        ],
    );
    let k = if dark { 0.55 } else { 0.42 };
    s.push_blur(70.0);
    blob(&s, W * 0.82, H * 0.12, 420.0, 0x00f2_8c38, k); // setting sun
    blob(&s, W * 0.18, H * 0.38, 380.0, 0x00c0_362c, k * 0.8); // International Orange
    blob(
        &s,
        W * 0.55,
        H * 1.02,
        520.0,
        0x002b_2d42,
        if dark { 0.9 } else { 0.30 },
    ); // the bay
    blob(&s, W * 0.95, H * 0.85, 300.0, 0x00c0_362c, k * 0.5);
    s.pop();
    s.push_blur(1.2);
    draw_bridge(&s, dark);
    s.pop();
    // Grain: a tiled faint noise texture gives the frosted look to the translucent cards above.
    let tile = graphene::Rect::new(0.0, 0.0, 128.0, 128.0);
    s.push_opacity(if dark { 0.05 } else { 0.10 });
    s.push_repeat(&bounds, Some(&tile));
    s.append_texture(&noise_texture(), &tile);
    s.pop();
    s.pop();
    s.to_node()
}

/// Renders the backdrop for `widget` (must be realized for a texture; a node paintable otherwise).
pub fn paint(picture: &gtk::Picture, dark: bool) {
    let Some(node) = node(dark) else {
        return;
    };
    let bounds = graphene::Rect::new(0.0, 0.0, W, H);
    let renderer = picture.native().and_then(|n| n.renderer());
    if let Some(r) = renderer {
        let tex = r.render_texture(&node, Some(&bounds));
        picture.set_paintable(Some(&tex));
    } else {
        let snap = gtk::Snapshot::new();
        snap.append_node(&node);
        picture.set_paintable(snap.to_paintable(Some(&graphene::Size::new(W, H))).as_ref());
    }
}
