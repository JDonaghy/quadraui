"""Runnable demo of `quadraui.app.widget.Widget`: a 5-star rating control
with its own click regions, keyboard handling and accessibility -- a
script-authored widget, not a composition of existing nodes (see
`AGENTS.md`'s "shape of an app" -- this is exactly the case composition
can't cover).

Run headless (clicks the fifth star, then nudges it with the keyboard,
printing what the app's own `on_rate` callback recorded after each):

    maturin develop
    python quadraui-py/examples/rating.py

Run in a real terminal (click a star, or focus the control with Tab and
use Left/Right to change the rating; `q`/Escape quits):

    python quadraui-py/examples/rating.py --tui
"""

from __future__ import annotations

import sys
from typing import Any, Dict, Optional

from quadraui.app import App, key_name
from quadraui.app.widget import Widget

WIDTH = 30
HEIGHT = 1
MAX_STARS = 5

_FILLED = {"r": 230, "g": 180, "b": 20, "a": 255}
_EMPTY = {"r": 120, "g": 120, "b": 120, "a": 255}


class RatingControl(Widget):
    """`props["value"]` (0..=`props["max"]`) is the current rating;
    `on_rate`, if given, is called with the new value on a star click or
    an arrow key while this control holds focus. Stateless itself (a
    fresh instance is built every `view()` call, same as any other node)
    -- the actual rating lives on whichever app object owns this."""

    def __init__(
        self,
        *,
        id: str,
        value: int,
        max_stars: int = MAX_STARS,
        on_rate: Optional[Any] = None,
    ) -> None:
        super().__init__(id=id, props={"value": value, "max": max_stars}, focusable=True)
        self.on_rate = on_rate

    def measure(self, ctx: Dict[str, Any], constraints: Dict[str, Any]) -> Dict[str, float]:
        width = self.props["max"] * 2 * ctx["char_width"]
        height = ctx["line_height"]
        max_width = constraints.get("max_width")
        if max_width is not None:
            width = min(width, max_width)
        return {"width": width, "height": height}

    def paint(self, ctx: Dict[str, Any], props: Dict[str, Any], rect: Dict[str, float]) -> Dict[str, Any]:
        max_stars = props["max"]
        value = props["value"]
        star_width = rect["width"] / max_stars
        ops = []
        regions = []
        for i in range(max_stars):
            x = i * star_width
            filled = i < value
            ops.append(
                {
                    "TextRun": {
                        "rect": {"x": x, "y": 0.0, "width": star_width, "height": rect["height"]},
                        "text": "★" if filled else "☆",
                        "color": _FILLED if filled else _EMPTY,
                    }
                }
            )
            regions.append(
                {
                    "id": f"star-{i}",
                    "rect": {"x": x, "y": 0.0, "width": star_width, "height": rect["height"]},
                }
            )
        return {"ops": ops, "regions": regions}

    def on_event(self, event: Dict[str, Any], props: Dict[str, Any], state: Dict[str, Any]) -> bool:
        canvas = event.get("Canvas")
        if canvas is not None:
            clicked = canvas.get("RegionClicked") or canvas.get("RegionDoubleClicked")
            if clicked is None:
                return False
            star = int(clicked["region"].split("-")[1])
            self._rate(star + 1, props)
            return True
        key = event.get("Key")
        if key is not None:
            name = key_name({"KeyPressed": {"key": key}})
            if name == "Right":
                self._rate(min(props["max"], props["value"] + 1), props)
                return True
            if name == "Left":
                self._rate(max(0, props["value"] - 1), props)
                return True
        return False

    def a11y(self, props: Dict[str, Any]) -> Optional[Dict[str, Any]]:
        return {
            "info": {
                "a11y_role": "Slider",
                "a11y_label": "Rating",
                "value": f"{props['value']} of {props['max']} stars",
                "value_range": {
                    "min": 0.0,
                    "max": float(props["max"]),
                    "current": float(props["value"]),
                },
            },
            "children": [],
        }

    def _rate(self, value: int, props: Dict[str, Any]) -> None:
        props["value"] = value
        if self.on_rate is not None:
            self.on_rate(value)


class _Quit(Exception):
    """Raised from `on_key` to unwind out of `run_tui()`."""


class RatingDemo:
    """Owns the one piece of state this demo needs: the current rating."""

    def __init__(self) -> None:
        self.rating = 3

    def _on_rate(self, value: int) -> None:
        self.rating = value

    def view(self) -> Any:
        # The sole root node, filling the whole viewport -- see
        # `RatingControl.measure`'s own doc for why this demo keeps it
        # that way rather than placing it alongside a text label in a
        # `Column` (every `Column` child defaults to an equal share of
        # the remaining space, not its own measured size, unless it opts
        # into `"size": "Content"` itself -- a composition wrinkle this
        # single-widget demo sidesteps rather than explains).
        return RatingControl(id="rating:stars", value=self.rating, on_rate=self._on_rate)

    def on_key(self, event: Any) -> None:
        name = key_name(event)
        if name in ("q", "Escape"):
            raise _Quit


def build() -> "tuple[App, RatingDemo]":
    demo = RatingDemo()
    app = App(WIDTH, HEIGHT, view=demo.view, on_event=demo.on_key)
    return app, demo


def _run_headless(app: App, demo: RatingDemo) -> None:
    print("before any interaction:")
    print(app.screen())
    print(f"rating == {demo.rating}")

    # The control is the sole root node, so it fills the whole
    # `WIDTH`-wide viewport: each of `MAX_STARS` stars is `WIDTH /
    # MAX_STARS` cells wide. Click the fifth star (index 4).
    star_width = WIDTH / MAX_STARS
    x = star_width * 4 + star_width / 2
    app.push_event({"MouseDown": {"button": "Left", "position": {"x": x, "y": 0.0}, "modifiers": {}}})
    app.run(1)
    print(f"\nafter clicking star 5, rating == {demo.rating}:")
    print(app.screen())

    # The click above already focused the control (its only tab stop),
    # so a plain `KeyPressed` routes straight to it.
    app.push_event({"KeyPressed": {"key": {"Named": "Left"}, "modifiers": {}, "repeat": False}})
    app.run(1)
    print(f"\nafter pressing Left, rating == {demo.rating}:")
    print(app.screen())


if __name__ == "__main__":
    app, demo = build()
    if "--tui" in sys.argv[1:]:
        try:
            app.run_tui()
        except _Quit:
            pass
        print(f"rating: {demo.rating}")
    else:
        _run_headless(app, demo)
