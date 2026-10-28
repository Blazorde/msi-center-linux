//! Step chart of a fan table (cairo on a `gtk::DrawingArea`). Geometry is in `msi_center::chart`.
//!
//! Every series differs by line style and a legend word as well as by colour: solid thick = steps
//! applied (or previewed), dashed = way down (hysteresis), dotted = exponential curve, hatched = S2 floor,
//! "▲" = step raised by S2, vertical line + dot = current temperature.

use std::cell::RefCell;
use std::fmt::Write as _;
use std::rc::Rc;

use gtk::cairo;
use gtk::prelude::*;
use msi_center::chart::{self, Frame, Padding};
use msi_center::curve::{CurveSpec, Generated, Table};
use msi_center::format::NNBSP;
use msi_center::stages;
use msi_core::staged::StagedCurve;

/// A staged CPU curve (ADR-0013), drawn instead of the 7-step table.
#[derive(Debug, Clone)]
pub struct StagedData {
    /// The curve shown (the draft being edited when it is valid, else the one in force).
    pub curve: StagedCurve,
    /// The curve in force, drawn thin under an unapplied preview.
    pub applied: Option<StagedCurve>,
    /// Whether `curve` is an unapplied preview (drawn dashed).
    pub preview: bool,
}

/// What to draw.
#[derive(Debug, Default, Clone)]
pub struct ChartData {
    /// Table currently in the EC.
    pub applied: Option<Generated>,
    /// Unapplied preview, drawn on top when present.
    pub preview: Option<Generated>,
    /// Parameters of the shown curve (for the dotted exponential).
    pub spec: Option<CurveSpec>,
    /// Current temperature.
    pub temp: Option<f64>,
    /// Staged CPU curve; when set it replaces the table and the exponential curve.
    pub staged: Option<StagedData>,
}

impl ChartData {
    fn shown(&self) -> Option<&Generated> {
        self.preview.as_ref().or(self.applied.as_ref())
    }
}

/// The chart widget.
#[derive(Debug, Clone)]
pub struct FanChart {
    /// Drawing area.
    pub area: gtk::DrawingArea,
    data: Rc<RefCell<ChartData>>,
}

type Rgb = (f64, f64, f64);
const ORANGE: Rgb = (0.949, 0.549, 0.220);
const RED: Rgb = (0.753, 0.212, 0.173);

fn path(cr: &cairo::Context, f: &Frame, pts: &[(f64, f64)]) {
    for (i, &p) in pts.iter().enumerate() {
        let (x, y) = f.point(p);
        if i == 0 {
            cr.move_to(x, y);
        } else {
            cr.line_to(x, y);
        }
    }
}

fn text(cr: &cairo::Context, s: &str, x: f64, y: f64, align: f64) {
    if let Ok(ext) = cr.text_extents(s) {
        cr.move_to(x - ext.width() * align, y);
        let _ = cr.show_text(s);
    }
}

#[allow(clippy::many_single_char_names, reason = "geometry")]
fn rounded(cr: &cairo::Context, x: f64, y: f64, w: f64, h: f64, r: f64) {
    use std::f64::consts::{FRAC_PI_2, PI};
    cr.new_sub_path();
    cr.arc(x + w - r, y + r, r, -FRAC_PI_2, 0.0);
    cr.arc(x + w - r, y + h - r, r, 0.0, FRAC_PI_2);
    cr.arc(x + r, y + h - r, r, FRAC_PI_2, PI);
    cr.arc(x + r, y + r, r, PI, 3.0 * FRAC_PI_2);
    cr.close_path();
}

impl FanChart {
    /// Builds an empty chart.
    pub fn new() -> Self {
        let data = Rc::new(RefCell::new(ChartData::default()));
        let area = gtk::DrawingArea::builder()
            .content_height(280)
            .hexpand(true)
            .accessible_role(gtk::AccessibleRole::Img)
            .build();
        area.update_property(&[gtk::accessible::Property::Label(
            "Courbe du ventilateur par paliers",
        )]);
        let d = data.clone();
        area.set_draw_func(move |a, cr, w, h| {
            draw(a, cr, f64::from(w), f64::from(h), &d.borrow());
        });
        Self { area, data }
    }

    /// Replaces what is drawn.
    pub fn update(&self, f: impl FnOnce(&mut ChartData)) {
        f(&mut self.data.borrow_mut());
        let desc = describe(&self.data.borrow());
        self.area
            .update_property(&[gtk::accessible::Property::Description(&desc)]);
        self.area.queue_draw();
    }
}

fn describe_staged(s: &StagedData, temp: Option<f64>) -> String {
    let c = &s.curve;
    let mut out = format!("Repos : {}{NNBSP}%", c.idle());
    for (i, st) in c.stages().iter().enumerate() {
        let _ = write!(
            out,
            " ; palier {} dès {}{NNBSP}°C : {}{NNBSP}%, retombée sous {}{NNBSP}°C",
            i + 1,
            st.up,
            st.speed,
            st.down
        );
    }
    if let Some(t) = temp {
        let _ = write!(
            out,
            ". Température actuelle {t:.0}{NNBSP}°C, {}{NNBSP}%.",
            stages::speed_at(c, t)
        );
    }
    out
}

fn describe(d: &ChartData) -> String {
    if let Some(s) = &d.staged {
        return describe_staged(s, d.temp);
    }
    let Some(g) = d.shown() else {
        return "Aucune courbe chargée.".to_owned();
    };
    let steps: Vec<String> = (0..7)
        .map(|i| {
            let from = if i == 0 {
                "repos".to_owned()
            } else {
                format!("dès {}{NNBSP}°C", g.table.up[i - 1])
            };
            let raised = if g.is_raised(i) { " (relevé S2)" } else { "" };
            format!("palier {i} {from} : {}{NNBSP}%{raised}", g.table.speeds[i])
        })
        .collect();
    let mut s = steps.join(" ; ");
    if let Some(t) = d.temp {
        let m = chart::marker(&g.table, t);
        let _ = write!(s, ". Température actuelle {t:.0}{NNBSP}°C, palier {}.", m.step);
    }
    s
}

#[allow(
    clippy::too_many_lines,
    clippy::many_single_char_names,
    reason = "one drawing routine, layered"
)]
fn draw(area: &gtk::DrawingArea, cr: &cairo::Context, w: f64, h: f64, d: &ChartData) {
    let Some(f) = Frame::new(w, h, Padding::CHART) else {
        return;
    };
    let fg = area.color();
    let (fr, fgc, fb) = (f64::from(fg.red()), f64::from(fg.green()), f64::from(fg.blue()));
    let ink = |a: f64| cr.set_source_rgba(fr, fgc, fb, a);
    cr.select_font_face("Sans", cairo::FontSlant::Normal, cairo::FontWeight::Normal);
    cr.set_font_size(11.0);

    // Grid and axes labels.
    cr.set_line_width(1.0);
    for t in chart::temp_ticks() {
        ink(0.08);
        cr.move_to(f.x(t).round() + 0.5, f.y0);
        cr.line_to(f.x(t).round() + 0.5, f.y0 + f.h);
        let _ = cr.stroke();
        ink(0.65);
        text(cr, &format!("{t:.0}°"), f.x(t), f.y0 + f.h + 18.0, 0.5);
    }
    for p in chart::PCT_TICKS {
        ink(0.08);
        cr.move_to(f.x0, f.y(p).round() + 0.5);
        cr.line_to(f.x0 + f.w, f.y(p).round() + 0.5);
        let _ = cr.stroke();
        ink(0.65);
        text(cr, &format!("{p:.0}{NNBSP}%"), f.x0 - 8.0, f.y(p) + 4.0, 1.0);
    }

    // S2 floor: hatched zone under the floor staircase.
    let floor = chart::floor_path();
    cr.save().ok();
    path(cr, &f, &floor);
    let last = floor.last().copied().unwrap_or((chart::T_MAX, 100.0));
    cr.line_to(f.x(last.0), f.y(0.0));
    cr.line_to(f.x(floor[0].0), f.y(0.0));
    cr.close_path();
    cr.clip();
    cr.set_source_rgba(RED.0, RED.1, RED.2, 0.07);
    cr.paint().ok();
    cr.set_source_rgba(RED.0, RED.1, RED.2, 0.22);
    cr.set_line_width(1.0);
    let mut x = f.x0 - f.h;
    while x < f.x0 + f.w {
        cr.move_to(x, f.y0 + f.h);
        cr.line_to(x + f.h, f.y0);
        x += 9.0;
    }
    let _ = cr.stroke();
    cr.restore().ok();
    cr.set_source_rgba(RED.0, RED.1, RED.2, 0.55);
    cr.set_line_width(1.2);
    cr.set_dash(&[2.0, 3.0], 0.0);
    path(cr, &f, &floor);
    let _ = cr.stroke();
    cr.set_dash(&[], 0.0);

    if let Some(s) = &d.staged {
        draw_staged(cr, &f, s, d.temp, (fr, fgc, fb));
        return;
    }

    // Exponential curve (dotted).
    if let Some(spec) = &d.spec {
        let pts: Vec<(f64, f64)> = (20u8..=100)
            .filter_map(|t| spec.curve_speed_at(t).map(|s| (f64::from(t), s)))
            .collect();
        if pts.len() > 1 {
            ink(0.45);
            cr.set_line_width(1.5);
            cr.set_dash(&[1.0, 4.0], 0.0);
            cr.set_line_cap(cairo::LineCap::Round);
            path(cr, &f, &pts);
            let _ = cr.stroke();
            cr.set_dash(&[], 0.0);
        }
    }

    // Applied table, thin, when a preview is drawn over it.
    if d.preview.is_some()
        && let Some(a) = &d.applied
    {
        ink(0.35);
        cr.set_line_width(1.5);
        path(cr, &f, &chart::rising_path(&a.table));
        let _ = cr.stroke();
    }

    let Some(g) = d.shown() else {
        return;
    };
    let table: &Table = &g.table;

    // Soft accent fill under the steps: reads as "how hard the fan works" at a glance.
    let rise = chart::rising_path(table);
    path(cr, &f, &rise);
    cr.line_to(f.x(chart::T_MAX), f.y(0.0));
    cr.line_to(f.x(chart::T_MIN), f.y(0.0));
    cr.close_path();
    let fill = cairo::LinearGradient::new(0.0, f.y0, 0.0, f.y0 + f.h);
    fill.add_color_stop_rgba(0.0, RED.0, RED.1, RED.2, 0.20);
    fill.add_color_stop_rgba(1.0, ORANGE.0, ORANGE.1, ORANGE.2, 0.02);
    let _ = cr.set_source(&fill);
    let _ = cr.fill();

    // Way down (dashed).
    cr.set_source_rgba(RED.0, RED.1, RED.2, 0.45);
    cr.set_line_width(1.5);
    cr.set_dash(&[5.0, 4.0], 0.0);
    path(cr, &f, &chart::falling_path(table));
    let _ = cr.stroke();
    cr.set_dash(&[], 0.0);

    // Steps (thick, sunset gradient).
    let grad = cairo::LinearGradient::new(f.x0, 0.0, f.x0 + f.w, 0.0);
    grad.add_color_stop_rgb(0.0, ORANGE.0, ORANGE.1, ORANGE.2);
    grad.add_color_stop_rgb(0.7, RED.0, RED.1, RED.2);
    grad.add_color_stop_rgb(1.0, 0.45, 0.12, 0.14);
    let _ = cr.set_source(&grad);
    cr.set_line_width(3.5);
    cr.set_line_join(cairo::LineJoin::Round);
    cr.set_line_cap(cairo::LineCap::Round);
    if d.preview.is_some() {
        cr.set_dash(&[10.0, 5.0], 0.0);
    }
    path(cr, &f, &chart::rising_path(table));
    let _ = cr.stroke();
    cr.set_dash(&[], 0.0);

    // Raised steps: "▲" above the step.
    cr.set_font_size(12.0);
    for (i, &(t, s)) in chart::step_centres(table).iter().enumerate() {
        if g.is_raised(i) {
            let (x, y) = f.point((t, s));
            cr.set_source_rgba(RED.0, RED.1, RED.2, 1.0);
            text(cr, "▲ S2", x, (y - 8.0).max(f.y0 + 10.0), 0.5);
        }
    }

    // Live marker.
    if let Some(temp) = d.temp {
        let m = chart::marker(table, temp);
        live_marker(cr, &f, (fr, fgc, fb), m.temp, m.speed);
    }
}

/// Vertical line, dot and label at the current temperature and the speed asked for there.
#[allow(clippy::many_single_char_names, reason = "geometry")]
fn live_marker(cr: &cairo::Context, f: &Frame, (fr, fgc, fb): Rgb, temp: f64, speed: f64) {
    let ink = |a: f64| cr.set_source_rgba(fr, fgc, fb, a);
    let (x, y) = f.point((temp, speed));
    ink(0.55);
    cr.set_line_width(1.0);
    cr.set_dash(&[3.0, 3.0], 0.0);
    cr.move_to(x.round() + 0.5, f.y0);
    cr.line_to(x.round() + 0.5, f.y0 + f.h);
    let _ = cr.stroke();
    cr.set_dash(&[], 0.0);
    cr.set_source_rgba(RED.0, RED.1, RED.2, 0.18);
    cr.arc(x, y, 13.0, 0.0, std::f64::consts::TAU);
    let _ = cr.fill();
    cr.set_source_rgba(1.0, 1.0, 1.0, 0.95);
    cr.arc(x, y, 7.0, 0.0, std::f64::consts::TAU);
    let _ = cr.fill();
    cr.set_source_rgba(RED.0, RED.1, RED.2, 1.0);
    cr.arc(x, y, 4.5, 0.0, std::f64::consts::TAU);
    let _ = cr.fill();
    cr.set_font_size(11.0);
    cr.select_font_face("Sans", cairo::FontSlant::Normal, cairo::FontWeight::Bold);
    let label = format!("{temp:.0}{NNBSP}°C · {speed:.0}{NNBSP}%");
    let align = if x > f.x0 + f.w * 0.75 { 1.0 } else { 0.0 };
    let dx = if align > 0.5 { -16.0 } else { 16.0 };
    let ty = (y - 16.0).max(f.y0 + 14.0);
    // Label on a small glass pill, legible over the grid and the hatching.
    if let Ok(ext) = cr.text_extents(&label) {
        let (pw, ph) = (ext.width() + 14.0, 20.0);
        let px = if align > 0.5 {
            x + dx - ext.width() - 7.0
        } else {
            x + dx - 7.0
        };
        let py = ty - 14.0;
        rounded(cr, px, py, pw, ph, ph / 2.0);
        let dark = (fr + fgc + fb) / 3.0 > 0.5;
        if dark {
            cr.set_source_rgba(0.11, 0.11, 0.17, 0.88);
        } else {
            cr.set_source_rgba(1.0, 1.0, 1.0, 0.9);
        }
        let _ = cr.fill();
    }
    ink(0.95);
    text(cr, &label, x + dx, ty, align);
}

/// The staged CPU curve: accent fill, dashed way down, thick stages, "▲ S2" where a floor takes over, and the
/// live marker. Same visual language as the step chart.
fn draw_staged(cr: &cairo::Context, f: &Frame, s: &StagedData, temp: Option<f64>, fg: Rgb) {
    let ink = |a: f64| cr.set_source_rgba(fg.0, fg.1, fg.2, a);

    // The curve in force, thin, when a preview is drawn over it.
    if s.preview
        && let Some(a) = &s.applied
    {
        ink(0.35);
        cr.set_line_width(1.5);
        path(cr, f, &stages::rising_path(a));
        let _ = cr.stroke();
    }

    // Soft accent fill under the stages.
    let rise = stages::rising_path(&s.curve);
    path(cr, f, &rise);
    cr.line_to(f.x(chart::T_MAX), f.y(0.0));
    cr.line_to(f.x(chart::T_MIN), f.y(0.0));
    cr.close_path();
    let fill = cairo::LinearGradient::new(0.0, f.y0, 0.0, f.y0 + f.h);
    fill.add_color_stop_rgba(0.0, RED.0, RED.1, RED.2, 0.20);
    fill.add_color_stop_rgba(1.0, ORANGE.0, ORANGE.1, ORANGE.2, 0.02);
    let _ = cr.set_source(&fill);
    let _ = cr.fill();

    // Way down (dashed): each stage is left at its own fall-back temperature.
    cr.set_source_rgba(RED.0, RED.1, RED.2, 0.45);
    cr.set_line_width(1.5);
    cr.set_dash(&[5.0, 4.0], 0.0);
    path(cr, f, &stages::falling_path(&s.curve));
    let _ = cr.stroke();
    cr.set_dash(&[], 0.0);

    // Stages (thick, sunset gradient).
    let grad = cairo::LinearGradient::new(f.x0, 0.0, f.x0 + f.w, 0.0);
    grad.add_color_stop_rgb(0.0, ORANGE.0, ORANGE.1, ORANGE.2);
    grad.add_color_stop_rgb(0.7, RED.0, RED.1, RED.2);
    grad.add_color_stop_rgb(1.0, 0.45, 0.12, 0.14);
    let _ = cr.set_source(&grad);
    cr.set_line_width(3.5);
    cr.set_line_join(cairo::LineJoin::Round);
    cr.set_line_cap(cairo::LineCap::Round);
    if s.preview {
        cr.set_dash(&[10.0, 5.0], 0.0);
    }
    path(cr, f, &rise);
    let _ = cr.stroke();
    cr.set_dash(&[], 0.0);

    // Floors that take over from the stages: "▲ S2" on the floor, never silent.
    cr.set_font_size(12.0);
    for r in s.curve.raised_floors() {
        let (x, y) = f.point((f64::from(r.floor.temp), f64::from(r.floor.speed)));
        cr.set_source_rgba(RED.0, RED.1, RED.2, 1.0);
        text(cr, "▲ S2", x, (y - 8.0).max(f.y0 + 10.0), 0.5);
    }

    if let Some(temp) = temp {
        live_marker(cr, f, fg, temp, f64::from(stages::speed_at(&s.curve, temp)));
    }
}
