//! Application state and behavior.
//!
//! [`App`] is a state machine: [`App::update`] applies an [`AppEvent`] (input,
//! timer tick, or the result of background work) and returns the [`Effect`]s
//! the runtime must perform (connect, fetch, kill, ...). It does no I/O, which
//! keeps it testable without a terminal or a MongoDB server.

mod input;
mod update;

#[cfg(test)]
mod tests;

use std::collections::HashSet;
use std::time::{Duration, Instant};

use crossterm::event::{KeyEvent, MouseEvent};
use ratatui::layout::Rect;
use ratatui::widgets::{ListState, TableState};

pub use input::TextInput;

use crate::logging::LogBuffer;
use crate::model::{
    FetchQuery, KillReport, KillRequest, NodeStatus, OpKey, Operation, ServerInfo, Snapshot,
};
use crate::theme::{self, Palette, Theme};

pub const MIN_REFRESH_INTERVAL: u64 = 1;
pub const MAX_REFRESH_INTERVAL: u64 = 10;
pub const DEFAULT_REFRESH_INTERVAL: u64 = 2;
/// Delay between the last filter keystroke and the refresh it triggers.
pub const FILTER_DEBOUNCE: Duration = Duration::from_millis(250);
/// A refresh shows the loading indicator once it takes longer than this, to
/// avoid flicker.
pub const LOADING_DELAY: Duration = Duration::from_millis(100);
pub const TOAST_DURATION: Duration = Duration::from_secs(5);
pub const ERROR_TOAST_DURATION: Duration = Duration::from_secs(8);
pub const MAX_TOASTS: usize = 5;

/// Clamps a refresh interval to the supported range.
pub fn clamp_refresh_interval(secs: i64) -> u64 {
    secs.clamp(MIN_REFRESH_INTERVAL as i64, MAX_REFRESH_INTERVAL as i64) as u64
}

/// Input to [`App::update`].
#[derive(Debug)]
pub enum AppEvent {
    Key(KeyEvent),
    Mouse(MouseEvent),
    Paste(String),
    Resize,
    /// Periodic timer: drives auto-refresh, debouncing and toast expiry.
    Tick,
    Connected(Result<ServerInfo, String>),
    Fetched {
        generation: u64,
        result: Result<Snapshot, String>,
    },
    Killed(KillReport),
}

/// Work requested by the application, performed by the runtime.
#[derive(Debug, Clone, PartialEq)]
pub enum Effect {
    /// (Re)connect to MongoDB; answered with [`AppEvent::Connected`].
    Connect,
    /// Fetch operations; answered with [`AppEvent::Fetched`] carrying the same
    /// generation. A newer fetch supersedes (and may cancel) older ones.
    Fetch {
        generation: u64,
        query: FetchQuery,
    },
    /// Kill operations; answered with [`AppEvent::Killed`].
    Kill(Vec<KillRequest>),
    /// Persist the theme choice.
    SaveTheme(&'static str),
    Quit,
}

/// User-triggerable actions, shared by key bindings, the footer and the help
/// screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Action {
    Help,
    Quit,
    Refresh,
    Kill,
    ToggleAutoRefresh,
    SortByTime,
    Logs,
    ToggleSelectAll,
    ToggleFilterBar,
    Theme,
    IncreaseInterval,
    DecreaseInterval,
    ToggleMongosLocal,
    Nodes,
}

impl Action {
    /// Key shown in the footer.
    pub fn key_label(self) -> &'static str {
        match self {
            Self::Help => "f1",
            Self::Quit => "^q",
            Self::Refresh => "^r",
            Self::Kill => "^k",
            Self::ToggleAutoRefresh => "^p",
            Self::SortByTime => "^s",
            Self::Logs => "^l",
            Self::ToggleSelectAll => "^a",
            Self::ToggleFilterBar => "^f",
            Self::Theme => "^t",
            Self::IncreaseInterval => "+",
            Self::DecreaseInterval => "-",
            Self::ToggleMongosLocal => "^o",
            Self::Nodes => "^n",
        }
    }

    /// Short description shown in the footer.
    pub fn description(self) -> &'static str {
        match self {
            Self::Help => "Help",
            Self::Quit => "Quit",
            Self::Refresh => "Refresh",
            Self::Kill => "Kill Selected",
            Self::ToggleAutoRefresh => "Pause/Resume",
            Self::SortByTime => "Sort by Time",
            Self::Logs => "View Logs",
            Self::ToggleSelectAll => "Toggle Selection",
            Self::ToggleFilterBar => "Toggle Filters",
            Self::Theme => "Theme",
            Self::IncreaseInterval => "Increase Interval",
            Self::DecreaseInterval => "Decrease Interval",
            Self::ToggleMongosLocal => "Mongos Ops",
            Self::Nodes => "Nodes",
        }
    }
}

/// Lines of the help screen: `(keys, description)`; an empty `keys` starts a
/// new section titled `description`.
pub const HELP: &[(&str, &str)] = &[
    ("", "Keyboard Shortcuts"),
    ("F1, ?", "Show this help"),
    ("Ctrl+Q, Ctrl+C", "Quit application"),
    (
        "Ctrl+R",
        "Refresh operations list (reconnect after a failure)",
    ),
    ("Ctrl+K", "Kill selected operations"),
    ("Ctrl+P", "Pause/Resume auto-refresh"),
    ("Ctrl+S", "Sort by running time"),
    ("Ctrl+L", "View application logs"),
    ("Ctrl+A", "Toggle selection (select all/deselect all)"),
    ("Ctrl+F, /", "Toggle filter bar visibility"),
    ("Ctrl+T", "Change theme"),
    (
        "Ctrl+O",
        "Show/hide the operations of mongos itself (sharded clusters)",
    ),
    (
        "Ctrl+N",
        "Cluster members and their polling status (--all-nodes)",
    ),
    ("+, Ctrl++", "Increase refresh interval"),
    ("-, Ctrl+-", "Decrease refresh interval"),
    ("Enter", "See operation details"),
    ("Space", "Select operations"),
    ("Tab, Shift+Tab", "Move between the table and the filters"),
    ("Esc", "Close dialogs, leave the filter bar"),
    ("", "Usage"),
    ("", "- Use arrow keys or mouse to navigate"),
    ("", "- Space/Click to select operations"),
    ("", "- Filter operations using the input fields"),
    ("", "- Clear filters with the Clear button"),
    ("", "- Confirm kill operations when prompted"),
];

/// Filter bar inputs, in display order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FilterField {
    OpId,
    Operation,
    RunningTime,
    Client,
    Description,
    EffectiveUsers,
}

impl FilterField {
    pub const ALL: [Self; 6] = [
        Self::OpId,
        Self::Operation,
        Self::RunningTime,
        Self::Client,
        Self::Description,
        Self::EffectiveUsers,
    ];

    pub fn index(self) -> usize {
        match self {
            Self::OpId => 0,
            Self::Operation => 1,
            Self::RunningTime => 2,
            Self::Client => 3,
            Self::Description => 4,
            Self::EffectiveUsers => 5,
        }
    }

    pub fn placeholder(self) -> &'static str {
        match self {
            Self::OpId => "OpId",
            Self::Operation => "Operation",
            Self::RunningTime => "Running Time ≥ sec",
            Self::Client => "Client",
            Self::Description => "Description",
            Self::EffectiveUsers => "Effective Users",
        }
    }
}

/// Which widget receives keyboard input when no modal is open.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Focus {
    Table,
    Filter(FilterField),
    ClearButton,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConnectionState {
    Connecting,
    Connected(ServerInfo),
    Failed(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    Info,
    Warning,
    Error,
}

/// A transient notification.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Toast {
    pub message: String,
    pub severity: Severity,
    pub expires_at: Instant,
}

/// Modal dialogs. Scroll offsets are requested positions: the UI clamps them
/// to the content when rendering.
#[derive(Debug, Clone)]
pub enum Modal {
    Help {
        scroll: u16,
    },
    Logs {
        /// First visible line.
        scroll: usize,
        /// Stick to the end as lines are added.
        follow: bool,
    },
    Details {
        op: Box<Operation>,
        scroll: u16,
    },
    KillConfirm {
        /// The selected operations as they were when the dialog opened: the
        /// MongoDB layer checks that each opid still belongs to the same
        /// operation before killing it.
        requests: Vec<KillRequest>,
        /// "Yes" has focus; "No" is focused by default.
        yes_focused: bool,
    },
    Theme {
        /// Highlighted theme index in [`theme::all`]; the highlighted theme is
        /// previewed.
        list: ListState,
        /// Theme to restore on cancel.
        original: &'static Theme,
    },
    Nodes {
        scroll: u16,
    },
}

/// Screen areas recorded while rendering, used to map mouse clicks.
#[derive(Debug, Clone, Default)]
pub struct LayoutCache {
    /// Data rows of the operations table (inside the borders, below the
    /// header). Row `i` of the viewport is operation `table.offset() + i`.
    pub table_body: Rect,
    /// Filter inputs in [`FilterField::ALL`] order, inner text areas.
    /// Zero-sized when the filter bar is hidden.
    pub filter_inputs: [Rect; 6],
    pub clear_button: Rect,
    /// Clickable footer entries.
    pub footer: Vec<(Rect, Action)>,
    /// Kill confirmation buttons.
    pub kill_yes: Rect,
    pub kill_no: Rect,
    /// Item area of the theme list. Row `i` is theme
    /// `theme_list_offset + i`.
    pub theme_list: Rect,
    pub theme_list_offset: usize,
    /// Area of the open modal.
    pub modal_area: Rect,
    /// Visible toasts and their index in [`App::toasts`]. Drawn over
    /// everything else, so they take clicks first.
    pub toasts: Vec<(Rect, usize)>,
    /// Visible height of the open modal's scrollable content (page size).
    pub modal_page: u16,
}

/// Timing of the last completed refresh.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RefreshStats {
    pub at: Instant,
    pub took: Duration,
    pub count: usize,
}

/// Options to create an [`App`].
#[derive(Debug, Clone)]
pub struct AppOptions {
    pub refresh_interval: u64,
    pub theme: &'static Theme,
    pub truecolor: bool,
    pub logs: LogBuffer,
    /// Shown in the header.
    pub title: String,
}

/// The whole application state.
#[derive(Debug)]
pub struct App {
    pub title: String,
    pub connection: ConnectionState,
    /// Displayed operations, in display order.
    pub operations: Vec<Operation>,
    pub selected: HashSet<OpKey>,
    /// Table cursor (`selected()` is the cursor row) and scroll offset.
    pub table: TableState,
    pub sort_ascending: bool,
    /// Filter inputs, in [`FilterField::ALL`] order.
    pub filter_inputs: [TextInput; 6],
    pub filter_bar_visible: bool,
    pub focus: Focus,
    pub modal: Option<Modal>,
    pub auto_refresh: bool,
    /// Seconds between refreshes.
    pub refresh_interval: u64,
    /// A refresh has been running for longer than [`LOADING_DELAY`].
    pub loading: bool,
    /// The last refresh matched more than [`crate::model::MAX_OPERATIONS`].
    pub truncated: bool,
    /// Cluster member status from the last refresh (`--all-nodes`).
    pub nodes: Vec<NodeStatus>,
    /// Warnings from the last refresh.
    pub warnings: Vec<String>,
    /// List the connected mongos' own operations too.
    pub show_mongos_local: bool,
    pub kill_in_progress: bool,
    pub theme: &'static Theme,
    pub truecolor: bool,
    pub toasts: Vec<Toast>,
    pub layout: LayoutCache,
    pub logs: LogBuffer,
    pub last_refresh: Option<RefreshStats>,
    /// Error of the last failed refresh, cleared on success.
    pub last_error: Option<String>,

    fetch_generation: u64,
    /// Generation and start time of the fetch in flight.
    fetch_in_flight: Option<(u64, Instant)>,
    /// When the next automatic refresh is due.
    next_refresh_at: Option<Instant>,
    /// A refresh was requested and starts on the next update.
    refresh_requested: bool,
    /// When the filter inputs last changed without a refresh yet.
    filters_changed_at: Option<Instant>,
    should_quit: bool,
}

impl App {
    pub fn new(options: AppOptions) -> Self {
        Self {
            title: options.title,
            connection: ConnectionState::Connecting,
            operations: Vec::new(),
            selected: HashSet::new(),
            table: TableState::default(),
            sort_ascending: true,
            filter_inputs: Default::default(),
            filter_bar_visible: false,
            focus: Focus::Table,
            modal: None,
            auto_refresh: true,
            refresh_interval: options
                .refresh_interval
                .clamp(MIN_REFRESH_INTERVAL, MAX_REFRESH_INTERVAL),
            loading: false,
            truncated: false,
            nodes: Vec::new(),
            warnings: Vec::new(),
            show_mongos_local: false,
            kill_in_progress: false,
            theme: options.theme,
            truecolor: options.truecolor,
            toasts: Vec::new(),
            layout: LayoutCache::default(),
            logs: options.logs,
            last_refresh: None,
            last_error: None,
            fetch_generation: 0,
            fetch_in_flight: None,
            next_refresh_at: None,
            refresh_requested: false,
            filters_changed_at: None,
            should_quit: false,
        }
    }

    /// Effects to perform at startup.
    pub fn start(&mut self) -> Vec<Effect> {
        vec![Effect::Connect]
    }

    pub fn should_quit(&self) -> bool {
        self.should_quit
    }

    /// Colors to render with; follows the theme being previewed in the theme
    /// picker.
    pub fn palette(&self) -> Palette {
        let theme = match &self.modal {
            Some(Modal::Theme { list, .. }) => list
                .selected()
                .and_then(|i| theme::all().get(i))
                .unwrap_or(self.theme),
            _ => self.theme,
        };
        theme.palette(self.truecolor)
    }

    /// The operation under the table cursor.
    pub fn cursor_operation(&self) -> Option<&Operation> {
        self.table.selected().and_then(|i| self.operations.get(i))
    }

    /// Connection info once connected.
    pub fn server_info(&self) -> Option<&ServerInfo> {
        match &self.connection {
            ConnectionState::Connected(info) => Some(info),
            _ => None,
        }
    }

    /// Whether to show the Shard and Node columns: connected to a sharded
    /// cluster or polling every member.
    pub fn topology_columns(&self) -> bool {
        self.server_info()
            .is_some_and(|info| info.is_sharded() || info.all_nodes)
    }

    /// Footer entries, in display order.
    pub fn footer_actions(&self) -> Vec<Action> {
        let mut actions = vec![
            Action::Help,
            Action::Quit,
            Action::Refresh,
            Action::Kill,
            Action::ToggleAutoRefresh,
            Action::SortByTime,
            Action::Logs,
            Action::ToggleSelectAll,
            Action::ToggleFilterBar,
            Action::Theme,
        ];
        if let Some(info) = self.server_info() {
            if info.is_sharded() {
                actions.push(Action::ToggleMongosLocal);
            }
            if info.all_nodes {
                actions.push(Action::Nodes);
            }
        }
        actions
    }

    /// Text of the status bar.
    pub fn status_text(&self) -> String {
        let connection = match &self.connection {
            ConnectionState::Connecting => "Connecting...".to_owned(),
            ConnectionState::Connected(info) => {
                format!("Connected to {} ({})", info.target, info.describe())
            }
            ConnectionState::Failed(_) => "Disconnected".to_owned(),
        };
        let refresh = if self.auto_refresh {
            "Auto-refresh enabled"
        } else {
            "Auto-refresh paused"
        };
        let mut text = format!("{connection} | {refresh} ({}s)", self.refresh_interval);
        if !self.selected.is_empty() {
            text.push_str(&format!(" | Selected: {}", self.selected.len()));
        }
        if !self.nodes.is_empty() {
            let ok = self.nodes.iter().filter(|n| n.health.is_ok()).count();
            text.push_str(&format!(" | Nodes: {ok}/{} ok", self.nodes.len()));
        }
        if self.show_mongos_local {
            text.push_str(" | +mongos ops");
        }
        if self.kill_in_progress {
            text.push_str(" | Killing...");
        }
        text
    }

    /// Title of the operations table.
    pub fn table_title(&self) -> String {
        let mut title = format!("Operations ({})", self.operations.len());
        if self.truncated {
            title.push_str(&format!(
                " • slowest {} shown",
                crate::model::MAX_OPERATIONS
            ));
        }
        if self.loading {
            title.push_str(" • Refreshing...");
        }
        title
    }

    /// Current filter values.
    pub fn filters(&self) -> crate::model::Filters {
        let value = |f: FilterField| self.filter_inputs[f.index()].value().trim().to_owned();
        crate::model::Filters {
            opid: value(FilterField::OpId),
            operation: value(FilterField::Operation),
            running_time: value(FilterField::RunningTime),
            client: value(FilterField::Client),
            description: value(FilterField::Description),
            effective_users: value(FilterField::EffectiveUsers),
        }
    }
}
