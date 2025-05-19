# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

Close MongoDB Operations Manager is a terminal-based UI tool for monitoring and managing MongoDB operations. The application allows users to view current operations running on a MongoDB server, filter them by various criteria, and kill long-running or problematic operations.

## Development Environment Setup

### Python Setup

The project uses [uv](https://docs.astral.sh/uv/) for Python environment management:

1. Install uv:
```shell
# Follow installation instructions at https://docs.astral.sh/uv/getting-started/installation/
```

2. Set up the Python environment:
```shell
# Install the required Python version
uv python install 3.13

# List Python versions
uv python list

# Pin to Python 3.13
uv python pin cpython-3.13.3-macos-aarch64-none
```

### Dependencies

Install dependencies:
```shell
uv sync
```

## Running the Application

Run the application:
```shell
# Using direct Python execution
uv run src/close_mongo_ops_manager/app.py [options]

# Or using uvx (easier)
uvx -n close-mongo-ops-manager [options]
```

### Command Line Options

The application supports these command line options:
- `--host`: MongoDB host (default: localhost or MONGODB_HOST env var)
- `--port`: MongoDB port (default: 27017 or MONGODB_PORT env var)
- `--username`: MongoDB username (or MONGODB_USERNAME env var)
- `--password`: MongoDB password (or MONGODB_PASSWORD env var)
- `--namespace`: MongoDB namespace to monitor (default: ".*")
- `--refresh-interval`: Refresh interval in seconds (default: 5)
- `--show-system-ops`: Show system operations (disabled by default)
- `--version`: Show version information
- `--help`: Show help information

## Architecture

The application is built using the [Textual](https://textual.textualize.io/) framework for terminal user interfaces and uses [PyMongo](https://pymongo.readthedocs.io/en/stable/) for MongoDB connectivity.

### Key Components

1. **MongoOpsManager (`app.py`)**: Main application class that orchestrates all components.

2. **MongoDBManager (`mongodb_manager.py`)**: Handles MongoDB connections and operations.
   - Connects to MongoDB
   - Gets current operations with filtering
   - Kills operations with verification

3. **OperationsView (`operations_view.py`)**: Displays the list of operations in a table.
   - Handles sorting by running time
   - Manages operation selection

4. **FilterBar (`filterbar.py`)**: Allows filtering operations by various criteria.
   - OpId, Operation, Running Time, Client, Description, etc.

5. **OperationDetailsScreen (`operation_details_screen.py`)**: Shows detailed information about a selected operation.

6. **KillConfirmation (`kill_confirmation_screen.py`)**: Confirmation dialog for killing operations.

7. **LogScreen (`log_screen.py`)**: Displays application logs.

8. **StatusBar (`statusbar.py`)**: Shows status information like connection state and refresh status.

## Core Features

- Connect to MongoDB servers (authenticated or unauthenticated)
- View active MongoDB operations in a table
- Filter operations by various criteria
- Sort operations by running time
- Select and kill problematic operations
- Auto-refresh with configurable interval
- View detailed operation information
- View application logs