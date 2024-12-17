# close-mongo-ops-manager
Monitor and kill MongoDB operations (Be advised that this is in a pre-alpha state. Use at your own risk.)

# Requirements

Install [uv](https://docs.astral.sh/uv/getting-started/installation/#installing-uv)

Once installed you should see something similar to this:
```shell
$ uv version
uv 0.5.4 (c62c83c37 2024-11-20)
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
uv python pin cpython-3.13.0-macos-aarch64-none
```

# Dependencies

Sync the project
```shell
uv sync
```

# Running the app

Launch the app with the required arguments
```shell
uv run app.py --help
```

![App screenshot](img/close-mongo-ops-manager.png "Close Mongo Ops Manager")
