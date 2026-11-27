#!/usr/bin/python3
"""Rasterise TUI capture SVGs (quadraui-gallery --capture, hero-capture) to PNG
with Pango font fallback.

Why not cairosvg: it uses cairo's toy text API (no per-glyph fallback), so every
codicon / Nerd-Font PUA glyph the TUI paints renders as tofu. Pango falls back
through fontconfig, so with "Symbols Nerd Font" installed (for example under
~/.local/share/fonts/) icons render like a Nerd-Font terminal.

Handles exactly the shape quadraui-gallery/src/capture.rs::write_tui_svg (and
examples/hero/rust/src/bin/capture.rs) emit: a root
<svg width height font-size>, full-size background <rect>, cell <rect>s and
<text x y textLength lengthAdjust="spacingAndGlyphs" fill [font-weight]> runs
(y = baseline). Each run is stretched/squeezed to textLength, matching
lengthAdjust=spacingAndGlyphs.

Needs system python3 with PyGObject + pangocairo + pycairo (Ubuntu:
python3-gi python3-gi-cairo gir1.2-pango-1.0).
Usage: /usr/bin/python3 scripts/tui_svg2png.py SRC_DIR DST_DIR [font]
  font default: "DejaVu Sans Mono" (CI-runner-like; the SVG's own
  family list is 'SF Mono','DejaVu Sans Mono',monospace).
"""
import json, os, sys
import xml.etree.ElementTree as ET
import gi
gi.require_version("Pango", "1.0")
gi.require_version("PangoCairo", "1.0")
from gi.repository import Pango, PangoCairo
import cairo

NS = "{http://www.w3.org/2000/svg}"


def hexrgb(s):
    s = s.lstrip("#")
    return tuple(int(s[i:i + 2], 16) / 255 for i in (0, 2, 4))


def render(svg_path, png_path, family):
    root = ET.parse(svg_path).getroot()
    w, h = int(float(root.get("width"))), int(float(root.get("height")))
    size = float(root.get("font-size", "14"))
    surf = cairo.ImageSurface(cairo.FORMAT_RGB24, w, h)
    cr = cairo.Context(surf)
    fo = cairo.FontOptions()
    fo.set_antialias(cairo.ANTIALIAS_GRAY)
    fo.set_hint_style(cairo.HINT_STYLE_SLIGHT)
    for el in root:
        tag = el.tag.replace(NS, "")
        if tag == "rect":
            x = el.get("x", "0"); y = el.get("y", "0")
            ww = el.get("width"); hh = el.get("height")
            ww = w if ww.endswith("%") else float(ww)
            hh = h if hh.endswith("%") else float(hh)
            cr.set_source_rgb(*hexrgb(el.get("fill", "#000000")))
            cr.rectangle(float(x), float(y), ww, hh)
            cr.fill()
        elif tag == "text":
            txt = el.text or ""
            if not txt.strip():
                continue
            x, y = float(el.get("x")), float(el.get("y"))
            tl = float(el.get("textLength", "0"))
            layout = PangoCairo.create_layout(cr)
            PangoCairo.context_set_font_options(layout.get_context(), fo)
            fd = Pango.FontDescription()
            fd.set_family(family)
            fd.set_absolute_size(size * Pango.SCALE)
            if el.get("font-weight") == "bold":
                fd.set_weight(Pango.Weight.BOLD)
            if el.get("font-style") == "italic":
                fd.set_style(Pango.Style.ITALIC)
            layout.set_font_description(fd)
            layout.set_text(txt, -1)
            _, logical = layout.get_pixel_extents()
            base = layout.get_baseline() / Pango.SCALE
            cr.save()
            cr.translate(x, y - base)
            if tl > 0 and logical.width > 0:
                cr.scale(tl / logical.width, 1.0)
            cr.set_source_rgb(*hexrgb(el.get("fill", "#ffffff")))
            PangoCairo.show_layout(cr, layout)
            cr.restore()
    surf.write_to_png(png_path)


def main():
    src, dst = sys.argv[1], sys.argv[2]
    family = sys.argv[3] if len(sys.argv) > 3 else "DejaVu Sans Mono"
    os.makedirs(dst, exist_ok=True)
    n = 0
    for name in sorted(os.listdir(src)):
        if name.endswith(".svg"):
            render(os.path.join(src, name), os.path.join(dst, name[:-4] + ".png"), family)
            n += 1
    mf = os.path.join(src, "manifest.json")
    if os.path.exists(mf):
        rows = json.load(open(mf))
        for r in rows:
            p = r.get("path") or ""
            if p.endswith(".svg"):
                r["svg_path"] = p
                r["path"] = p[:-4] + ".png"
        json.dump(rows, open(os.path.join(dst, "manifest.json"), "w"), indent=1)
    print(f"rasterised {n} svg -> png into {dst} (font {family!r})")


if __name__ == "__main__":
    main()
