from quadraui.app import App, Button, Column, Row, Text


class Counter:
    def __init__(self):
        self.count = 0

    def view(self):
        return Column(
            Text(f"Count: {self.count}", id="count"),
            Row(Button("-", id="dec", on_click=self.dec), Button("+", id="inc", on_click=self.inc)),
        )

    def inc(self):
        self.count += 1

    def dec(self):
        self.count -= 1


if __name__ == "__main__":
    App(view=Counter().view).run_tui()  # q quits; or .run_native(title="Counter"): same Counter
