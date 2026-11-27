from quadraui.app import App, Button, Column, Form, List, Row, StatusBar, TextInputField, key_name


def item(text):
    return {"text": {"spans": [{"text": text, "fg": None, "bg": None}]}}


def segment(text):
    return {"text": f" {text} ", "fg": {"r": 220, "g": 220, "b": 220, "a": 255}, "bg": {"r": 40, "g": 40, "b": 60, "a": 255}}


class Todo:
    def __init__(self):
        self.draft = ""
        self.todos = ["Write the README", "Ship 0.12"]
        self.selected = 0
        self.log = []  # every update, in order: what the parity runner compares

    def view(self):
        return Column(
            Form([TextInputField(self.draft, id="draft", label="New", on_change=self.edit, on_submit=self.add)], id="form"),
            List([item(t) for t in self.todos], id="todos", on_select=self.select),
            Row(Button("Add", id="add", on_click=self.add), Button("Clear", id="clear", on_click=self.clear)),
            StatusBar([segment(f"{len(self.todos)} todos")], [segment(f"selected {self.selected}")], id="status"),
        )

    def edit(self, text):
        self.log.append(f"edit {text}")
        self.draft = text

    def add(self, *_):
        self.log.append(f"add {self.draft}")
        if self.draft:
            self.todos.append(self.draft)
            self.draft = ""

    def select(self, index):
        self.log.append(f"select {index}")
        self.selected = index

    def clear(self):
        self.log.append("clear")
        self.todos, self.selected = [], 0

    def on_key(self, event):
        if key_name(event) == "Escape":
            raise SystemExit


if __name__ == "__main__":
    todo = Todo()
    App(40, 12, view=todo.view, on_event=todo.on_key).run_tui()
