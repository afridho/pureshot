// Compositor-side rounding for trusted PureShot settings/editor windows only.
// Independent implementation: no imports or runtime dependency on RWC Reborn.
import Cogl from 'gi://Cogl';
import GObject from 'gi://GObject';
import Shell from 'gi://Shell';

export const WindowCorners = GObject.registerClass(class PureShotWindowCorners extends Shell.GLSLEffect {
    vfunc_build_pipeline() {
        this.add_glsl_snippet(Cogl.SnippetHook.FRAGMENT, `
            uniform vec4 psBounds;
            uniform vec2 psTextureSize;
            uniform float psRadius;
        `, `
            vec2 pixel = cogl_tex_coord_in[0].xy * psTextureSize;
            vec2 center = (psBounds.xy + psBounds.zw) * 0.5;
            vec2 halfSize = (psBounds.zw - psBounds.xy) * 0.5;
            vec2 q = abs(pixel - center) - halfSize + vec2(psRadius);
            float distance = length(max(q, vec2(0.0))) + min(max(q.x, q.y), 0.0) - psRadius;
            cogl_color_out *= 1.0 - smoothstep(-0.75, 0.75, distance);
        `, false);
    }

    update(actor, window) {
        const frame = window.get_frame_rect();
        const buffer = window.get_buffer_rect();
        const width = actor.get_width();
        const height = actor.get_height();
        if (width <= 0 || height <= 0 || buffer.width <= 0 || buffer.height <= 0)
            return;
        // Frame excludes invisible client borders; normalize to the actor's
        // texture coordinates rather than assuming integer monitor scaling.
        const sx = width / buffer.width;
        const sy = height / buffer.height;
        const x = (frame.x - buffer.x) * sx;
        const y = (frame.y - buffer.y) * sy;
        const radius = window.is_fullscreen() || window.maximizedHorizontally || window.maximizedVertically ? 0 :
            Math.min(20 * sx, frame.width * sx / 2, frame.height * sy / 2);
        this.set_uniform_float(this.get_uniform_location('psTextureSize'), 2, [width, height]);
        this.set_uniform_float(this.get_uniform_location('psBounds'), 4,
            [x, y, x + frame.width * sx, y + frame.height * sy]);
        this.set_uniform_float(this.get_uniform_location('psRadius'), 1, [radius]);
        this.queue_repaint();
    }
});

export function shouldRoundWindow(title) {
    return title === 'PureShot' || title === 'PureShot Annotate' ||
        title.startsWith('PureShot — ');
}

export function installWindowCorners(service, global) {
    const tracked = new Map();
    const attach = actor => {
        const window = actor.meta_window;
        if (!window || tracked.has(window) || !shouldRoundWindow(window.get_title() ?? ''))
            return;
        try {
            if (!service._allowedExecutables().includes(service._executable(window.get_pid())))
                return;
            const effect = new WindowCorners();
            actor.add_effect_with_name('pureshot-window-corners', effect);
            const update = () => effect.update(actor, window);
            const signals = [
                [actor, actor.connect('notify::size', update)],
                [window, window.connect('size-changed', update)],
                [window, window.connect('notify::fullscreen', update)],
                [window, window.connect('notify::maximized-horizontally', update)],
                [window, window.connect('notify::maximized-vertically', update)],
            ];
            const cleanup = () => {
                for (const [object, id] of signals)
                    object.disconnect(id);
                actor.remove_effect(effect);
                tracked.delete(window);
            };
            signals.push([window, window.connect('unmanaged', cleanup)]);
            tracked.set(window, cleanup);
            update();
        } catch (error) {
            console.warn(`PureShot window corners: ${error.message}`);
        }
    };
    const mapId = global.window_manager.connect('map', (_wm, actor) => attach(actor));
    for (const actor of global.get_window_actors())
        attach(actor);
    return () => {
        global.window_manager.disconnect(mapId);
        for (const cleanup of [...tracked.values()])
            cleanup();
    };
}
