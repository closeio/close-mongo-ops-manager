# Message classes for better component communication
from textual.message import Message
from dataclasses import dataclass


@dataclass
class FilterChanged(Message):
    """Filter criteria changed."""

    filters: dict[str, str]


@dataclass
class OperationsLoaded(Message):
    """Event emitted when operations are fully loaded."""

    count: int
    duration: float


@dataclass
class SelectionChanged(Message):
    """Event emitted when the selection of operations changes."""

    count: int
