//! Terminal setup and the event loop: feeds terminal input, timer ticks and
//! background results to the [`App`], and performs the effects it returns.

use std::any::Any;
use std::future::Future;
use std::io::{self, Stdout};
use std::panic::AssertUnwindSafe;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Once};
use std::time::{Duration, Instant};

use anyhow::Context;
use crossterm::event::{
    DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture, Event,
    EventStream, KeyEventKind, KeyboardEnhancementFlags, MouseEventKind,
    PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
};
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
    supports_keyboard_enhancement,
};
use crossterm::{cursor, execute};
use futures::{FutureExt, StreamExt, stream};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use tokio::sync::mpsc;
use tokio::task::{JoinHandle, JoinSet};
use tokio::time::MissedTickBehavior;

use crate::app::{App, AppEvent, AppOptions, Effect};
use crate::config::ConfigStore;
use crate::model::{FetchQuery, KillOutcome, KillReport, KillRequest, Snapshot};
use crate::mongo::{ConnectConfig, MongoManager};
use crate::ui;

/// Timer resolution: auto-refresh, debouncing, toasts and the loading
/// indicator are driven by ticks.
const TICK: Duration = Duration::from_millis(100);
/// Kills performed concurrently.
const KILL_CONCURRENCY: usize = 8;
/// Upper bound for closing connections on exit.
const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(3);

/// Everything needed to run the application.
pub struct RuntimeConfig {
    pub connect: ConnectConfig,
    pub app: AppOptions,
    pub config_store: Option<ConfigStore>,
}

/// Runs the application until the user quits.
pub async fn run(config: RuntimeConfig) -> anyhow::Result<()> {
    let mut session = TerminalSession::start().context("failed to set up the terminal")?;
    let result = event_loop(&mut session, config).await;
    session.stop();
    result
}

async fn event_loop(session: &mut TerminalSession, config: RuntimeConfig) -> anyhow::Result<()> {
    let (tx, mut rx) = mpsc::unbounded_channel();
    let mut workers = Workers::new(config.connect, config.config_store, tx);
    let mut app = App::new(config.app);
    let mut effects = app.start();
    let mut input = EventStream::new();
    let mut ticker = tokio::time::interval(TICK);
    ticker.set_missed_tick_behavior(MissedTickBehavior::Skip);
    let shutdown = shutdown_signal();
    tokio::pin!(shutdown);

    // Input the app ignores (mouse motion, ...) doesn't trigger a redraw.
    let mut redraw = true;
    let result = loop {
        for effect in effects.drain(..) {
            workers.perform(effect);
        }
        if app.should_quit() {
            break Ok(());
        }
        if redraw && let Err(error) = session.draw(&mut app) {
            break Err(anyhow::Error::new(error).context("failed to draw the screen"));
        }
        redraw = false;
        let event = tokio::select! {
            event = input.next() => match event {
                Some(Ok(event)) => match translate(event) {
                    Some(event) => event,
                    None => continue,
                },
                Some(Err(error)) => {
                    break Err(anyhow::Error::new(error).context("failed to read terminal input"));
                }
                None => break Ok(()),
            },
            Some(message) = rx.recv() => match workers.receive(message) {
                Some(event) => event,
                None => continue,
            },
            _ = ticker.tick() => AppEvent::Tick,
            () = &mut shutdown => {
                log::info!("Received a termination signal");
                break Ok(());
            }
        };
        effects = app.update(event, Instant::now());
        redraw = true;
    };
    // Give the terminal back right away; closing connections can take a
    // moment.
    session.stop();
    workers.shutdown().await;
    result
}

/// Converts terminal input into application events; `None` for input the
/// application doesn't use.
fn translate(event: Event) -> Option<AppEvent> {
    match event {
        Event::Key(key) if key.kind != KeyEventKind::Release => Some(AppEvent::Key(key)),
        Event::Mouse(mouse)
            if matches!(
                mouse.kind,
                MouseEventKind::Down(_) | MouseEventKind::ScrollUp | MouseEventKind::ScrollDown
            ) =>
        {
            Some(AppEvent::Mouse(mouse))
        }
        Event::Paste(text) => Some(AppEvent::Paste(text)),
        Event::Resize(..) => Some(AppEvent::Resize),
        _ => None,
    }
}

/// Results of background work.
enum Message {
    Connected {
        attempt: u64,
        result: Result<Arc<MongoManager>, String>,
    },
    Fetched {
        generation: u64,
        result: Result<Snapshot, String>,
    },
    Killed(KillReport),
}

/// Performs effects in background tasks and turns their results into
/// application events.
struct Workers {
    connect_config: ConnectConfig,
    store: Option<ConfigStore>,
    tx: mpsc::UnboundedSender<Message>,
    manager: Option<Arc<MongoManager>>,
    /// Increases with every connection attempt; results of superseded
    /// attempts are discarded.
    connect_attempt: u64,
    connect_task: Option<JoinHandle<()>>,
    fetch_task: Option<JoinHandle<()>>,
    kill_tasks: JoinSet<()>,
}

impl Workers {
    fn new(
        connect_config: ConnectConfig,
        store: Option<ConfigStore>,
        tx: mpsc::UnboundedSender<Message>,
    ) -> Self {
        Self {
            connect_config,
            store,
            tx,
            manager: None,
            connect_attempt: 0,
            connect_task: None,
            fetch_task: None,
            kill_tasks: JoinSet::new(),
        }
    }

    fn perform(&mut self, effect: Effect) {
        match effect {
            Effect::Connect => self.connect(),
            Effect::Fetch { generation, query } => self.fetch(generation, query),
            Effect::Kill(requests) => self.kill(requests),
            Effect::SaveTheme(name) => self.save_theme(name),
            Effect::Quit => {}
        }
    }

    fn connect(&mut self) {
        abort(self.connect_task.take());
        abort(self.fetch_task.take());
        if let Some(previous) = self.manager.take() {
            tokio::spawn(async move { previous.shutdown().await });
        }
        self.connect_attempt += 1;
        let attempt = self.connect_attempt;
        let config = self.connect_config.clone();
        let tx = self.tx.clone();
        self.connect_task = Some(tokio::spawn(async move {
            let result = catch_panic(MongoManager::connect(config))
                .await
                .and_then(|r| r.map_err(|e| e.to_string()))
                .map(Arc::new);
            let _ = tx.send(Message::Connected { attempt, result });
        }));
    }

    fn fetch(&mut self, generation: u64, query: FetchQuery) {
        // A newer fetch supersedes the one in flight.
        abort(self.fetch_task.take());
        let tx = self.tx.clone();
        let Some(manager) = self.manager.clone() else {
            let _ = tx.send(Message::Fetched {
                generation,
                result: Err("not connected".to_owned()),
            });
            return;
        };
        self.fetch_task = Some(tokio::spawn(async move {
            let result = catch_panic(async move { manager.fetch(&query).await })
                .await
                .and_then(|r| r.map_err(|e| e.to_string()));
            let _ = tx.send(Message::Fetched { generation, result });
        }));
    }

    fn kill(&mut self, requests: Vec<KillRequest>) {
        while self.kill_tasks.try_join_next().is_some() {}
        let tx = self.tx.clone();
        let Some(manager) = self.manager.clone() else {
            let results = requests
                .into_iter()
                .map(|r| (r, KillOutcome::Failed("not connected".to_owned())))
                .collect();
            let _ = tx.send(Message::Killed(KillReport { results }));
            return;
        };
        self.kill_tasks.spawn(async move {
            let results = stream::iter(requests)
                .map(|request| {
                    let manager = Arc::clone(&manager);
                    async move {
                        let outcome = catch_panic(async { manager.kill(&request).await })
                            .await
                            .unwrap_or_else(KillOutcome::Failed);
                        (request, outcome)
                    }
                })
                .buffered(KILL_CONCURRENCY)
                .collect()
                .await;
            let _ = tx.send(Message::Killed(KillReport { results }));
        });
    }

    fn save_theme(&self, name: &str) {
        let Some(store) = &self.store else {
            return;
        };
        if let Err(error) = store.save_theme(name) {
            log::warn!(
                "Failed to save theme config to {}: {error}",
                store.path().display()
            );
        }
    }

    fn receive(&mut self, message: Message) -> Option<AppEvent> {
        Some(match message {
            Message::Connected { attempt, result } => {
                if attempt != self.connect_attempt {
                    if let Ok(stale) = result {
                        tokio::spawn(async move { stale.shutdown().await });
                    }
                    return None;
                }
                self.connect_task = None;
                AppEvent::Connected(result.map(|manager| {
                    let info = manager.info().clone();
                    self.manager = Some(manager);
                    info
                }))
            }
            Message::Fetched { generation, result } => AppEvent::Fetched { generation, result },
            Message::Killed(report) => AppEvent::Killed(report),
        })
    }

    async fn shutdown(&mut self) {
        abort(self.connect_task.take());
        abort(self.fetch_task.take());
        self.kill_tasks.abort_all();
        if let Some(manager) = self.manager.take()
            && tokio::time::timeout(SHUTDOWN_TIMEOUT, manager.shutdown())
                .await
                .is_err()
        {
            log::warn!("Timed out closing MongoDB connections");
        }
    }
}

fn abort(task: Option<JoinHandle<()>>) {
    if let Some(task) = task {
        task.abort();
    }
}

/// Runs `future`, turning a panic into an error so the application keeps
/// getting answers to its requests.
async fn catch_panic<T>(future: impl Future<Output = T>) -> Result<T, String> {
    AssertUnwindSafe(future)
        .catch_unwind()
        .await
        .map_err(|panic| {
            let message = panic_message(panic.as_ref());
            log::error!("Internal error: {message}");
            format!("internal error: {message}")
        })
}

fn panic_message(panic: &(dyn Any + Send)) -> String {
    panic
        .downcast_ref::<&str>()
        .map(|s| (*s).to_owned())
        .or_else(|| panic.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "panic".to_owned())
}

/// Resolves on SIGTERM, SIGHUP or SIGINT.
async fn shutdown_signal() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{Signal, SignalKind, signal};

        async fn recv(signal: Option<Signal>) {
            match signal {
                Some(mut signal) => {
                    signal.recv().await;
                }
                None => std::future::pending().await,
            }
        }

        let terminate = signal(SignalKind::terminate()).ok();
        let hangup = signal(SignalKind::hangup()).ok();
        let interrupt = signal(SignalKind::interrupt()).ok();
        tokio::select! {
            () = recv(terminate) => {}
            () = recv(hangup) => {}
            () = recv(interrupt) => {}
        }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
}

/// Whether the terminal is set up for the application. Restoring it happens
/// once, whichever comes first: a normal exit, the session being dropped, or
/// a panic.
static TERMINAL_ACTIVE: AtomicBool = AtomicBool::new(false);
/// Whether keyboard enhancement flags were pushed and must be popped.
static KEYBOARD_ENHANCED: AtomicBool = AtomicBool::new(false);

/// The terminal in raw mode on the alternate screen, with mouse capture.
/// Restored on drop and on panics.
struct TerminalSession {
    terminal: Terminal<CrosstermBackend<Stdout>>,
}

impl TerminalSession {
    fn start() -> io::Result<Self> {
        install_panic_hook();
        enable_raw_mode()?;
        TERMINAL_ACTIVE.store(true, Ordering::SeqCst);
        let mut stdout = io::stdout();
        if let Err(error) = execute!(
            stdout,
            EnterAlternateScreen,
            EnableMouseCapture,
            EnableBracketedPaste
        ) {
            restore_terminal();
            return Err(error);
        }
        // Lets terminals that support it report Ctrl+= / Ctrl+- and Esc
        // unambiguously.
        if matches!(supports_keyboard_enhancement(), Ok(true))
            && execute!(
                stdout,
                PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES)
            )
            .is_ok()
        {
            KEYBOARD_ENHANCED.store(true, Ordering::SeqCst);
        }
        match Terminal::new(CrosstermBackend::new(stdout)) {
            Ok(terminal) => Ok(Self { terminal }),
            Err(error) => {
                restore_terminal();
                Err(error)
            }
        }
    }

    fn draw(&mut self, app: &mut App) -> io::Result<()> {
        self.terminal.draw(|frame| ui::draw(frame, app))?;
        Ok(())
    }

    fn stop(&mut self) {
        restore_terminal();
    }
}

impl Drop for TerminalSession {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Leaves raw mode and the alternate screen, once. Errors are ignored: this
/// runs on the way out.
fn restore_terminal() {
    if !TERMINAL_ACTIVE.swap(false, Ordering::SeqCst) {
        return;
    }
    let mut stdout = io::stdout();
    if KEYBOARD_ENHANCED.swap(false, Ordering::SeqCst) {
        let _ = execute!(stdout, PopKeyboardEnhancementFlags);
    }
    let _ = execute!(
        stdout,
        DisableBracketedPaste,
        DisableMouseCapture,
        LeaveAlternateScreen,
        cursor::Show
    );
    let _ = disable_raw_mode();
}

/// Restores the terminal before a panic on the main thread is reported.
/// Panics in background tasks are only logged: they are turned into errors
/// by [`catch_panic`] and the UI keeps running.
fn install_panic_hook() {
    static INSTALL: Once = Once::new();
    INSTALL.call_once(|| {
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            log::error!("{info}");
            let main_thread = std::thread::current().name() == Some("main");
            if main_thread || !TERMINAL_ACTIVE.load(Ordering::SeqCst) {
                restore_terminal();
                previous(info);
            }
        }));
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseEvent};

    #[test]
    fn translate_keeps_relevant_input() {
        let key = KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE);
        assert!(matches!(translate(Event::Key(key)), Some(AppEvent::Key(_))));
        let mut release = key;
        release.kind = KeyEventKind::Release;
        assert!(translate(Event::Key(release)).is_none());

        let mouse = |kind| MouseEvent {
            kind,
            column: 1,
            row: 1,
            modifiers: KeyModifiers::NONE,
        };
        assert!(translate(Event::Mouse(mouse(MouseEventKind::Moved))).is_none());
        assert!(translate(Event::Mouse(mouse(MouseEventKind::ScrollDown))).is_some());
        assert!(matches!(
            translate(Event::Paste("x".into())),
            Some(AppEvent::Paste(_))
        ));
        assert!(matches!(
            translate(Event::Resize(1, 1)),
            Some(AppEvent::Resize)
        ));
        assert!(translate(Event::FocusGained).is_none());
    }

    #[tokio::test]
    async fn panics_become_errors() {
        let result: Result<(), String> = catch_panic(async { panic!("boom") }).await;
        assert_eq!(result.unwrap_err(), "internal error: boom");
        assert_eq!(catch_panic(async { 7 }).await, Ok(7));
    }

    #[test]
    fn panic_messages() {
        let boxed: Box<dyn Any + Send> = Box::new(String::from("owned"));
        assert_eq!(panic_message(boxed.as_ref()), "owned");
        let boxed: Box<dyn Any + Send> = Box::new(42_u8);
        assert_eq!(panic_message(boxed.as_ref()), "panic");
    }
}
