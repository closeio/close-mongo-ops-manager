"""Tests for FilterBar component."""

from textual.app import App, ComposeResult
from textual.widgets import Input

from close_mongo_ops_manager.filterbar import FilterBar
from close_mongo_ops_manager.messages import FilterChanged


class FilterBarTestApp(App):
    """Test app with a FilterBar."""

    def __init__(self):
        super().__init__()
        self.filter_messages: list[dict] = []

    def compose(self) -> ComposeResult:
        yield FilterBar()

    def on_filter_changed(self, event: FilterChanged) -> None:
        self.filter_messages.append(event.filters)


async def test_filterbar_renders_all_inputs():
    """Test that FilterBar renders all expected input fields."""
    app = FilterBarTestApp()
    async with app.run_test():
        filter_bar = app.query_one(FilterBar)
        inputs = filter_bar.query(".filter-input")
        assert len(inputs) == 6


async def test_filterbar_input_posts_filter_changed():
    """Test that typing in an input posts a FilterChanged message."""
    app = FilterBarTestApp()
    async with app.run_test() as pilot:
        opid_input = app.query_one("#filter-opid", Input)
        opid_input.focus()
        await pilot.pause(0.1)
        opid_input.value = "12345"
        await pilot.pause(0.1)
        # Should have received at least one FilterChanged with opid
        matching = [f for f in app.filter_messages if f.get("opid") == "12345"]
        assert len(matching) > 0


async def test_filterbar_empty_inputs_excluded():
    """Test that empty input values are excluded from filter dict."""
    app = FilterBarTestApp()
    async with app.run_test() as pilot:
        opid_input = app.query_one("#filter-opid", Input)
        opid_input.focus()
        await pilot.pause(0.1)
        # Type something then clear it
        opid_input.value = "x"
        await pilot.pause(0.1)
        opid_input.value = ""
        await pilot.pause(0.1)
        # Last message should have no filters
        assert app.filter_messages[-1] == {}


async def test_filterbar_multiple_filters_combine():
    """Test that multiple filter inputs combine into one filter dict."""
    app = FilterBarTestApp()
    async with app.run_test() as pilot:
        app.query_one("#filter-opid", Input).value = "123"
        app.query_one("#filter-operation", Input).value = "query"
        await pilot.pause(0.1)
        # Find a message containing both filters
        matching = [
            f
            for f in app.filter_messages
            if f.get("opid") == "123" and f.get("operation") == "query"
        ]
        assert len(matching) > 0


async def test_filterbar_clear_button_resets_all():
    """Test that the Clear button resets all inputs and posts empty filters."""
    app = FilterBarTestApp()
    async with app.run_test() as pilot:
        # Set some filter values
        app.query_one("#filter-opid", Input).value = "123"
        app.query_one("#filter-client", Input).value = "10.0"
        await pilot.pause(0.1)

        # Click clear button
        await pilot.click("#clear-filters")
        await pilot.pause(0.1)

        # All inputs should be empty
        for inp in app.query_one(FilterBar).query(".filter-input"):
            if isinstance(inp, Input):
                assert inp.value == ""

        # Should have posted an empty filter dict
        assert {} in app.filter_messages


async def test_filterbar_id_to_key_conversion():
    """Test that filter input IDs are correctly converted to filter keys."""
    app = FilterBarTestApp()
    async with app.run_test() as pilot:
        # The running-time input should produce key "running_time"
        app.query_one("#filter-running-time", Input).value = "10"
        await pilot.pause(0.1)
        matching = [f for f in app.filter_messages if "running_time" in f]
        assert len(matching) > 0
        assert matching[-1]["running_time"] == "10"


async def test_filterbar_effective_users_key():
    """Test that effective users input produces correct filter key."""
    app = FilterBarTestApp()
    async with app.run_test() as pilot:
        app.query_one("#filter-effective-users", Input).value = "admin"
        await pilot.pause(0.1)
        matching = [f for f in app.filter_messages if "effective_users" in f]
        assert len(matching) > 0
