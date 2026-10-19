#!/usr/bin/env python3
"""Render the real preview CSS using WebKitGTK; requires a graphical session and PyGObject.

Run: python3 scripts/test_linux_preview.py
No capture permissions, app restart, or Cargo build is needed.
"""
import json
import sys
from pathlib import Path

import gi

gi.require_version("Gtk", "3.0")
gi.require_version("WebKit2", "4.1")
from gi.repository import GLib, Gtk, WebKit2

root = Path(__file__).resolve().parents[1]
window = Gtk.Window()
window.set_default_size(336, 210)
web = WebKit2.WebView()
window.add(web)
window.show_all()
css = (root / "src/styles.css").read_text()
results = []
errors = []


def fail(error):
    errors.append(str(error))
    Gtk.main_quit()
    return False


def check(index=0):
    width, height = [(2000, 100), (100, 2000), (400, 400)][index]
    script = """(() => {
        const img = document.querySelector('.preview img');
        img.src = 'data:image/svg+xml,' + encodeURIComponent(
            '<svg xmlns="http://www.w3.org/2000/svg" width="%s" height="%s"/>');
        const rect = document.querySelector('.preview').getBoundingClientRect();
        return JSON.stringify({width: rect.width, height: rect.height,
            ratio: rect.width / rect.height, fit: getComputedStyle(img).objectFit});
    })()""" % (width, height)

    def done(view, result):
        try:
            value = json.loads(view.evaluate_javascript_finish(result).to_string())
            assert abs(value["width"] - 336) < .1, value
            assert abs(value["height"] - 210) < .1, value
            assert abs(value["ratio"] - 1.6) < .001, value
            assert value["fit"] == "cover", value
            results.append(value)
            if index < 2:
                GLib.timeout_add(200, lambda: check(index + 1))
            else:
                check_close()
        except Exception as error:
            fail(error)

    web.evaluate_javascript(script, -1, None, None, None, done)
    return False


def check_close():
    script = """(() => {
        const button = document.querySelector('.close');
        const rect = button.getBoundingClientRect();
        const x = rect.x + rect.width / 2, y = rect.y + rect.height / 2;
        const hit = document.elementFromPoint(x, y);
        button.onclick = () => button.dataset.clicked = 'yes';
        hit.click();
        return JSON.stringify({visible: getComputedStyle(button).opacity,
            inside: rect.x >= 0 && rect.y >= 0 && rect.right <= innerWidth && rect.bottom <= innerHeight,
            clicked: button.dataset.clicked === 'yes', height: document.querySelector('.preview').offsetHeight});
    })()"""

    def done(view, result):
        try:
            value = json.loads(view.evaluate_javascript_finish(result).to_string())
            assert value['visible'] == '1' and value['inside'] and value['clicked'], value
            assert value['height'] == 210, value
            check_pin()
        except Exception as error:
            fail(error)

    # Wait for the last SVG's intrinsic dimensions to affect layout: this caught
    # the old height:auto regression that immediate DOM measurements missed.
    GLib.timeout_add(300, lambda: (web.evaluate_javascript(script, -1, None, None, None, done), False)[1])


def check_pin():
    script = """(() => {
        document.querySelector('.quick-overlay').classList.add('is-pinned');
        document.querySelector('.preview').classList.add('pinned-preview');
        const rect = document.querySelector('.preview').getBoundingClientRect();
        return JSON.stringify({width: rect.width, height: rect.height,
            fit: getComputedStyle(document.querySelector('img')).objectFit,
            aspect: getComputedStyle(document.querySelector('.preview')).aspectRatio});
    })()"""

    def done(view, result):
        try:
            value = json.loads(view.evaluate_javascript_finish(result).to_string())
            assert value['fit'] == 'contain', value
            assert value['aspect'] == 'auto', value
            Gtk.main_quit()
        except Exception as error:
            fail(error)

    web.evaluate_javascript(script, -1, None, None, None, done)


def loaded(view, event):
    if event == WebKit2.LoadEvent.FINISHED:
        GLib.timeout_add(400, check)


web.connect("load-changed", loaded)
web.load_html('<style>' + css + '</style><div class="capture-webview">'
              '<div class="quick-overlay"><div class="preview"><img><button class="close">×</button></div></div></div>',
              "http://localhost/")
GLib.timeout_add_seconds(15, lambda: fail("Timed out waiting for WebKitGTK"))
Gtk.main()
window.destroy()
if errors:
    print("FAIL:", *errors, file=sys.stderr)
    sys.exit(1)
print("PASS: landscape, portrait, square captures fill the 16:10 canvas with cover; pins retain contain fit")
print(json.dumps(results, indent=2))
