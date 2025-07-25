from textual.binding import Binding
from textual.app import ComposeResult
from textual.containers import Vertical, Center
from textual.screen import ModalScreen
from textual.widgets import Footer, Static, OptionList
from textual.widgets.option_list import Option


class ThemeScreen(ModalScreen):
    """Theme selection screen."""

    DEFAULT_CSS = """
    ThemeScreen {
        align: center middle;
    }
    
    #theme-container {
        width: 60;
        height: 20;
        border: round $primary;
        background: $surface;
        padding: 1;
    }
    
    #theme-title {
        text-align: center;
        text-style: bold;
        margin-bottom: 1;
    }
    
    #theme-list {
        height: 1fr;
        border: solid $primary;
        margin-top: 1;
    }
    """

    BINDINGS = [
        Binding("escape", "dismiss", "Cancel", show=False),
        Binding("enter", "select_theme", "Select Theme", show=False),
    ]

    def __init__(self, available_themes: list[str], current_theme: str):
        super().__init__()
        self.available_themes = available_themes
        self.current_theme = current_theme

    def compose(self) -> ComposeResult:
        yield Footer()
        with Center():
            with Vertical(id="theme-container"):
                yield Static("Select Theme:", id="theme-title")
                options = []
                for theme in self.available_themes:
                    marker = " ✓" if theme == self.current_theme else ""
                    display_name = theme.replace("-", " ").title()
                    options.append(Option(f"{display_name}{marker}", id=theme))
                yield OptionList(*options, id="theme-list")

    def on_mount(self) -> None:
        """Focus the option list when the screen mounts."""
        option_list = self.query_one("#theme-list", OptionList)
        option_list.focus()

        # Highlight the current theme by default
        try:
            for index in range(len(self.available_themes)):
                if self.available_themes[index] == self.current_theme:
                    option_list.highlighted = index
                    break
        except Exception:
            # If highlighting fails, just focus the first item
            if len(self.available_themes) > 0:
                option_list.highlighted = 0

    def action_select_theme(self) -> None:
        """Select the highlighted theme."""
        option_list = self.query_one("#theme-list", OptionList)
        if option_list.highlighted is not None:
            highlighted_option = option_list.get_option_at_index(
                option_list.highlighted
            )
            if highlighted_option:
                selected_theme = highlighted_option.id
                self.dismiss(selected_theme)
            else:
                self.dismiss(None)
        else:
            # If no option is highlighted, dismiss without selection
            self.dismiss(None)

    def on_option_list_option_selected(self, event: OptionList.OptionSelected) -> None:
        """Handle theme selection."""
        self.dismiss(event.option.id)
