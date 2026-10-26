// SPDX-License-Identifier: GPL-3.0-or-later
// Image thumbnails for the clipboard popup: clipd `Get` → async GdkPixbuf decode at thumbnail size →
// St.ImageContent. LRU cache capped in decoded bytes (`thumbnail-cache-mb`), concurrency-limited.
import Cogl from 'gi://Cogl';
import GdkPixbuf from 'gi://GdkPixbuf';
import Gio from 'gi://Gio';
import St from 'gi://St';

const THUMB_W = 112;
const THUMB_H = 72;
const MAX_PARALLEL = 2;

export class ThumbCache {
    /**
     * @param {import('./clipd.js').ClipdClient} clipd
     * @param {Gio.Settings} settings
     */
    constructor(clipd, settings) {
        this._clipd = clipd;
        this._settings = settings;
        this._entries = new Map(); // id → {content, bytes}  (Map order = LRU order)
        this._pending = new Map(); // id → Promise
        this._queue = [];
        this._running = 0;
        this._total = 0;
        this._cancellable = new Gio.Cancellable();
    }

    get _cap() {
        return Math.max(1, this._settings.get_uint('thumbnail-cache-mb')) * 1024 * 1024;
    }

    /** @returns {St.ImageContent|null} cached content, refreshed in LRU order */
    peek(id) {
        const e = this._entries.get(id);
        if (!e)
            return null;
        this._entries.delete(id);
        this._entries.set(id, e);
        return e.content;
    }

    /**
     * @param {number} id clip id
     * @param {number} size clip size in bytes (skip over MaxClipBytes)
     * @returns {Promise<St.ImageContent|null>}
     */
    load(id, size) {
        const hit = this.peek(id);
        if (hit)
            return Promise.resolve(hit);
        if (size > this._clipd.maxClipBytes)
            return Promise.resolve(null);
        if (this._pending.has(id))
            return this._pending.get(id);
        const p = new Promise(resolve => {
            this._queue.push({id, resolve});
            this._pump();
        });
        this._pending.set(id, p);
        p.finally(() => this._pending.delete(id));
        return p;
    }

    _pump() {
        while (this._running < MAX_PARALLEL && this._queue.length > 0) {
            const {id, resolve} = this._queue.shift();
            this._running++;
            this._decode(id).then(resolve, e => {
                console.debug(`msi-center: thumbnail ${id}: ${e.message}`);
                resolve(null);
            }).finally(() => {
                this._running--;
                this._pump();
            });
        }
    }

    async _decode(id) {
        const {bytes} = await this._clipd.get(id);
        const scale = St.ThemeContext.get_for_stage(global.stage).scale_factor;
        const stream = Gio.MemoryInputStream.new_from_bytes(bytes);
        const pixbuf = await new Promise((resolve, reject) => {
            GdkPixbuf.Pixbuf.new_from_stream_at_scale_async(stream, THUMB_W * scale, THUMB_H * scale, true,
                this._cancellable, (_src, res) => {
                    try {
                        resolve(GdkPixbuf.Pixbuf.new_from_stream_finish(res));
                    } catch (e) {
                        reject(e);
                    }
                });
        });
        const coglContext = global.stage.context.get_backend().get_cogl_context();
        const content = St.ImageContent.new_with_preferred_size(pixbuf.width / scale, pixbuf.height / scale);
        content.set_bytes(coglContext, pixbuf.read_pixel_bytes(),
            pixbuf.has_alpha ? Cogl.PixelFormat.RGBA_8888 : Cogl.PixelFormat.RGB_888,
            pixbuf.width, pixbuf.height, pixbuf.rowstride);
        const cost = pixbuf.rowstride * pixbuf.height;
        this._entries.set(id, {content, bytes: cost});
        this._total += cost;
        this._evict();
        return content;
    }

    _evict() {
        for (const [id, e] of this._entries) {
            if (this._total <= this._cap)
                break;
            this._entries.delete(id);
            this._total -= e.bytes;
        }
    }

    /** Drop entries for deleted clips. */
    forget(ids) {
        for (const id of ids) {
            const e = this._entries.get(id);
            if (e) {
                this._entries.delete(id);
                this._total -= e.bytes;
            }
        }
    }

    destroy() {
        this._cancellable.cancel();
        this._queue = [];
        this._entries.clear();
        this._total = 0;
    }
}
