// SPDX-License-Identifier: GPL-3.0-or-later
// Live mini sparkline: CPU % (filled, sunset gradient) and CPU °C (thin line), last ~60 s of Telemetry.
// Colors come from stylesheet.css (`-msi-spark-*` properties) so light/dark stay in CSS.
import Cairo from 'cairo';
import GObject from 'gi://GObject';
import St from 'gi://St';

const TEMP_MIN = 30;
const TEMP_MAX = 100;

export const Sparkline = GObject.registerClass(
class MsiSparkline extends St.DrawingArea {
    _init() {
        super._init({style_class: 'msi-sparkline', x_expand: true});
        this._cpu = [];
        this._temp = [];
        this._colors = null;
    }

    setSeries(cpu, temp) {
        this._cpu = cpu.slice();
        this._temp = temp.slice();
        this.queue_repaint();
    }

    vfunc_style_changed() {
        const node = this.get_theme_node();
        this._colors = {
            top: node.get_color('-msi-spark-top'),
            bottom: node.get_color('-msi-spark-bottom'),
            line: node.get_color('-msi-spark-line'),
            temp: node.get_color('-msi-spark-temp'),
        };
        super.vfunc_style_changed();
    }

    vfunc_repaint() {
        const cr = this.get_context();
        const [w, h] = this.get_surface_size();
        if (!this._colors || w <= 0 || h <= 0) {
            cr.$dispose();
            return;
        }
        const pad = 2;
        const plot = (series, lo, hi) => {
            const n = series.length;
            const step = n > 1 ? (w - 2 * pad) / (60 - 1) : 0;
            const x0 = w - pad - step * (n - 1);
            return series.map((v, i) => {
                const f = Math.min(1, Math.max(0, (v - lo) / (hi - lo)));
                return [x0 + i * step, h - pad - f * (h - 2 * pad)];
            });
        };

        const cpu = plot(this._cpu, 0, 100);
        if (cpu.length > 1) {
            const {top, bottom, line} = this._colors;
            const grad = new Cairo.LinearGradient(0, 0, 0, h);
            grad.addColorStopRGBA(0, top.red / 255, top.green / 255, top.blue / 255, top.alpha / 255);
            grad.addColorStopRGBA(1, bottom.red / 255, bottom.green / 255, bottom.blue / 255, bottom.alpha / 255);
            cr.moveTo(cpu[0][0], h);
            for (const [x, y] of cpu)
                cr.lineTo(x, y);
            cr.lineTo(cpu[cpu.length - 1][0], h);
            cr.closePath();
            cr.setSource(grad);
            cr.fill();

            cr.setLineWidth(1.5);
            cr.setLineJoin(Cairo.LineJoin.ROUND);
            cr.moveTo(...cpu[0]);
            for (const [x, y] of cpu.slice(1))
                cr.lineTo(x, y);
            cr.setSourceColor(line);
            cr.stroke();
        }

        const temp = plot(this._temp, TEMP_MIN, TEMP_MAX);
        if (temp.length > 1) {
            cr.setLineWidth(1.2);
            cr.setDash([3, 3], 0);
            cr.moveTo(...temp[0]);
            for (const [x, y] of temp.slice(1))
                cr.lineTo(x, y);
            cr.setSourceColor(this._colors.temp);
            cr.stroke();
        }
        cr.$dispose();
    }
});
