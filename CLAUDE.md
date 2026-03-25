# CLAUDE.md

## Project Overview

Close MongoDB Operations Manager — terminal UI (Textual + PyMongo) for monitoring and killing MongoDB operations.

## Commands

```shell
# Install dependencies (dev + lint)
uv sync --python 3.13 --all-groups

# Run tests
uv run pytest -v

# Run a single test file / test
uv run pytest tests/test_app.py -v
uv run pytest tests/test_app.py::TestClassName::test_method -v

# Lint & format
uv run ruff check src/ tests/
uv run ruff format src/ tests/

# Run the app
uv run src/close_mongo_ops_manager/app.py [options]
```

## Architecture

Source: `src/close_mongo_ops_manager/`

| File | Purpose |
|------|---------|
| `app.py` | Main app class (MongoOpsManager), orchestrates everything |
| `mongodb_manager.py` | MongoDB connection, fetching ops, killing ops |
| `operations_view.py` | DataTable displaying operations, sorting by run time |
| `filterbar.py` | Filter bar with inputs for OpId, Operation, Client, etc. |
| `operation_details_screen.py` | Detail view for a selected operation |
| `kill_confirmation_screen.py` | Confirmation dialog before killing an op |
| `log_screen.py` | Application log viewer |
| `statusbar.py` | Connection state and refresh status display |
| `theme_manager.py` | Theme registry and persistence |
| `theme_screen.py` | Interactive theme picker (Ctrl+T) |
| `config_manager.py` | Config persistence at `~/.config/close-mongo-ops-manager/config.json` |
| `help_screen.py` | Help/keybindings screen |
| `messages.py` | Custom Textual messages between components |
| `exceptions.py` | Custom exception classes |

## Code Style

- Ruff for linting and formatting (line-length=88, target py312)
- Python >=3.12

## Testing

- pytest with pytest-asyncio (`asyncio_mode = auto` in pytest.ini)
- Tests in `tests/`, fixtures in `tests/conftest.py`
- Textual app tests use `async` test functions
