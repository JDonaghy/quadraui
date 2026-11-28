from quadraui.app import App, Button, Column, Row, TextDisplay, key_name


def text(s):
    return [{"spans": [{"text": s, "fg": None, "bg": None}]}]


class Counter:
    def __init__(self):
        self.count = 0

    def view(self):
        return Column(
            TextDisplay(text(f"Count: {self.count}"), id="count"),
            Row(Button("-", id="dec", on_click=self.dec), Button("+", id="inc", on_click=self.inc)),
        )

    def inc(self):
        self.count += 1

    def dec(self):
        self.count -= 1

    def on_key(self, event):
        if key_name(event) == "q":
            raise SystemExit


if __name__ == "__main__":
    counter = Counter()
    App(30, 5, view=counter.view, on_event=counter.on_key).run_tui()  # or .run_native(title="Counter"): same Counter
