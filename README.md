# close-mongo-ops-manager
Monitor and kill MongoDB operations (Be advised that this is in a Beta state. You may encounter some bugs.)

The app is also published in [pypi](https://pypi.org/project/close-mongo-ops-manager/)

# Requirements

Install [uv](https://docs.astral.sh/uv/getting-started/installation/#installing-uv)

Once installed you should see something similar to this:
```shell
$ uv self version
uv 0.7.19 (38ee6ec80 2025-07-02)
```

Use the right Python version
```shell
uv python install 3.13
```

List the Python versions
```shell
uv python list
```

Pin the Python 3.13 version
```shell
uv python pin cpython-3.13.3-macos-aarch64-none
```

# Dependencies

Sync the project
```shell
uv sync
```

# Running the app

Launch the application
```shell
uv run src/close_mongo_ops_manager/app.py --help
```

Or you can just use `uvx`
```shell
uvx -n close-mongo-ops-manager
```

## Usage

These are the actions you can do in the app. You can see them in the app help menu as well.
```
f1      : Show this help
Ctrl+Q  : Quit application
Ctrl+R  : Refresh operations list
Ctrl+K  : Kill selected operations
Ctrl+P  : Pause/Resume auto-refresh
Ctrl+S  : Sort by running time
Ctrl+L  : View application logs
Ctrl+A  : Toggle selection (select all/deselect all)
Ctrl+F  : Toggle filter bar visibility
Ctrl++  : Increase refresh interval
Ctrl+-  : Decrease refresh interval
Enter   : See operation details
Space   : Select operations
```

The mouse is enabled, so all menus are clickable.

Take into account that the auto-refresh is disabled by default. If you enable it to refresh the operations automatically, when you find the operation you want to kill you need to stop it first. Then select the operation and kill it.

The selected operations are not preserved between refreshes. This will improve in the future releases.
There is also a known issue about in the filter bar that looses focus.

This is the tipical usage:

- Use arrow keys or mouse to navigate
- Space/Click to select operations
- Filter operations using the input fields
- Clear filters with the Clear button
- Confirm kill operations when prompted

## Screenshot

![App screenshot](img/close-mongo-ops-manager.png "Close Mongo Ops Manager")
