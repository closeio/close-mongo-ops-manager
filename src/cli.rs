//! Command line interface.

use std::io::IsTerminal;
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;

use clap::parser::ValueSource;
use clap::{ArgMatches, CommandFactory, FromArgMatches, Parser};
use log::LevelFilter;
use mongodb::options::{ConnectionString, Credential, ServerAddress};

use crate::app::{self, AppOptions};
use crate::config::ConfigStore;
use crate::logging;
use crate::mongo::{ConnectConfig, ConnectTarget};
use crate::runtime::{self, RuntimeConfig};
use crate::theme::{self, Theme};

/// Default log file, in the working directory (as in the Python version).
pub const LOG_FILE: &str = "close_mongo_ops_manager.log";

const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Close MongoDB Operations Manager: monitor and kill MongoDB operations.
#[derive(Debug, Clone, Parser)]
#[command(
    name = "close-mongo-ops-manager",
    version = concat!("v", env!("CARGO_PKG_VERSION")),
    about = concat!("Close MongoDB Operations Manager v", env!("CARGO_PKG_VERSION"), ": monitor and kill MongoDB operations"),
    max_term_width = 100
)]
pub struct Args {
    /// MongoDB connection string (mongodb:// or mongodb+srv://). Takes
    /// precedence over --host and --port; TLS and other options go here.
    #[arg(long, env = "MONGODB_URI", value_name = "URI", hide_env_values = true)]
    pub uri: Option<String>,

    /// MongoDB host. A comma-separated seed list of host[:port] is accepted.
    /// The default is the IPv4 loopback address, where mongod listens by
    /// default: "localhost" may resolve to ::1 first, which another local
    /// user could listen on.
    #[arg(long, env = "MONGODB_HOST", default_value = "127.0.0.1")]
    pub host: String,

    /// MongoDB port (for hosts given without one).
    #[arg(long, env = "MONGODB_PORT", default_value_t = 27017)]
    pub port: u16,

    /// MongoDB username.
    #[arg(long, env = "MONGODB_USERNAME")]
    pub username: Option<String>,

    /// MongoDB password.
    #[arg(long, env = "MONGODB_PASSWORD", hide_env_values = true)]
    pub password: Option<String>,

    /// MongoDB authentication database.
    #[arg(long, env = "MONGODB_AUTH_SOURCE", default_value = "admin")]
    pub auth_source: String,

    /// MongoDB namespace to monitor: only operations whose namespace starts
    /// with it (case-insensitive) are listed.
    #[arg(long, default_value = "")]
    pub namespace: String,

    /// Refresh interval in seconds (min: 1, max: 10).
    #[arg(
        long,
        env = "MONGODB_REFRESH_INTERVAL",
        default_value_t = app::DEFAULT_REFRESH_INTERVAL as i64,
        allow_negative_numbers = true
    )]
    pub refresh_interval: i64,

    /// Show system operations (hidden by default).
    #[arg(long)]
    pub show_system_ops: bool,

    /// Enable load balancer support for MongoDB connections.
    #[arg(long)]
    pub load_balanced: bool,

    /// Discover every cluster member (shard and config server replica set
    /// members, replica set secondaries) and poll each one directly, so
    /// operations on secondaries and config servers are listed too.
    #[arg(long, env = "MONGODB_ALL_NODES")]
    pub all_nodes: bool,

    /// Username for direct connections to cluster members with --all-nodes.
    /// Users created through mongos only exist on the config servers, so
    /// shard members may need shard-local users. Defaults to --username.
    #[arg(long, env = "MONGODB_NODE_USERNAME")]
    pub node_username: Option<String>,

    /// Password for --node-username.
    #[arg(long, env = "MONGODB_NODE_PASSWORD", hide_env_values = true)]
    pub node_password: Option<String>,

    /// Authentication database for --node-username. Defaults to --auth-source.
    #[arg(long, env = "MONGODB_NODE_AUTH_SOURCE")]
    pub node_auth_source: Option<String>,

    /// Timeout in seconds for connecting and for every server round trip.
    #[arg(
        long,
        env = "MONGODB_TIMEOUT",
        default_value_t = 5,
        value_parser = clap::value_parser!(u64).range(1..=300)
    )]
    pub timeout: u64,

    /// Log file (replaced by a new owner-only file at startup).
    #[arg(long, env = "CLOSE_MONGO_OPS_MANAGER_LOG_FILE", default_value = LOG_FILE)]
    pub log_file: PathBuf,

    /// Log debug details.
    #[arg(short, long)]
    pub verbose: bool,
}

/// Entry point.
pub fn main() -> ExitCode {
    let matches = Args::command().get_matches();
    let mut args = match Args::from_arg_matches(&matches) {
        Ok(args) => args,
        Err(error) => error.exit(),
    };
    let level = if args.verbose {
        LevelFilter::Debug
    } else {
        LevelFilter::Info
    };
    let (logs, log_error) = logging::init(&args.log_file, level);
    if let Some(error) = &log_error {
        eprintln!(
            "Warning: cannot write the log file {}: {error}",
            args.log_file.display()
        );
    }
    log::info!("Starting Close MongoDB Operations Manager v{VERSION}");
    if let Some(warning) = prefer_command_line_host(&mut args, &matches) {
        log::warn!("{warning}");
    }

    if !std::io::stdin().is_terminal() || !std::io::stdout().is_terminal() {
        eprintln!("Error: close-mongo-ops-manager needs an interactive terminal");
        return ExitCode::FAILURE;
    }

    let connect = match connect_config(&args) {
        Ok(config) => config,
        Err(error) => {
            log::error!("{error}");
            eprintln!("Error: {error}");
            return ExitCode::from(2);
        }
    };

    let refresh_interval = refresh_interval(args.refresh_interval);
    let store = ConfigStore::default_location();
    let options = AppOptions {
        refresh_interval,
        theme: load_theme(store.as_ref()),
        truecolor: theme::truecolor_supported(),
        logs,
        title: format!("Close MongoDB Operations Manager v{VERSION}"),
    };

    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .worker_threads(
            std::thread::available_parallelism()
                .map_or(2, |n| n.get())
                .clamp(2, 4),
        )
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            eprintln!("Error: failed to start the async runtime: {error}");
            return ExitCode::FAILURE;
        }
    };
    let result = runtime.block_on(runtime::run(RuntimeConfig {
        connect,
        app: options,
        config_store: store,
    }));
    // Don't wait for driver background tasks (monitors) to wind down.
    runtime.shutdown_timeout(Duration::from_secs(1));

    match result {
        Ok(()) => {
            log::info!("Exiting Close MongoDB Operations Manager. Hasta luego!");
            log::logger().flush();
            ExitCode::SUCCESS
        }
        Err(error) => {
            log::error!("Startup error: {error:#}");
            log::logger().flush();
            eprintln!("\nError: {error:#}");
            eprintln!("Please check {} for details", args.log_file.display());
            ExitCode::FAILURE
        }
    }
}

/// `MONGODB_URI` is a common variable, often exported for other tools: when
/// it is set but `--host`/`--port` are given on the command line, the command
/// line wins. Returns a warning to log when the variable is ignored.
fn prefer_command_line_host(args: &mut Args, matches: &ArgMatches) -> Option<String> {
    let from_command_line = |id: &str| matches.value_source(id) == Some(ValueSource::CommandLine);
    if args.uri.is_some()
        && matches.value_source("uri") == Some(ValueSource::EnvVariable)
        && (from_command_line("host") || from_command_line("port"))
    {
        args.uri = None;
        return Some(
            "Ignoring MONGODB_URI: --host/--port were given on the command line".to_owned(),
        );
    }
    None
}

/// Clamps the refresh interval, logging when it was out of range.
fn refresh_interval(requested: i64) -> u64 {
    let interval = app::clamp_refresh_interval(requested);
    if requested < app::MIN_REFRESH_INTERVAL as i64 {
        log::warn!(
            "Refresh interval too low, setting to minimum ({} seconds)",
            app::MIN_REFRESH_INTERVAL
        );
    } else if requested > app::MAX_REFRESH_INTERVAL as i64 {
        log::warn!(
            "Refresh interval too high, setting to maximum ({} seconds)",
            app::MAX_REFRESH_INTERVAL
        );
    }
    interval
}

/// The saved theme, or the default one.
fn load_theme(store: Option<&ConfigStore>) -> &'static Theme {
    let Some(store) = store else {
        return theme::default_theme();
    };
    match store.load_theme() {
        Ok(Some(name)) => theme::by_name(&name).unwrap_or_else(|| {
            log::warn!("Unknown theme {name:?} in {}", store.path().display());
            theme::default_theme()
        }),
        Ok(None) => theme::default_theme(),
        Err(error) => {
            log::warn!(
                "Failed to load theme config from {}: {error}",
                store.path().display()
            );
            theme::default_theme()
        }
    }
}

/// Builds the connection settings from the arguments.
pub fn connect_config(args: &Args) -> Result<ConnectConfig, String> {
    let uri = args
        .uri
        .as_deref()
        .map(str::trim)
        .filter(|u| !u.is_empty())
        .or_else(|| {
            let host = args.host.trim();
            (host.starts_with("mongodb://") || host.starts_with("mongodb+srv://")).then_some(host)
        });
    let target = match uri {
        Some(uri) => {
            // The driver's message can quote parts of the URI, password
            // included: don't show it.
            ConnectionString::parse(uri).map_err(|_| {
                "invalid connection string (special characters in the username and password \
                 must be percent-encoded)"
                    .to_owned()
            })?;
            ConnectTarget::Uri(uri.to_owned())
        }
        None => ConnectTarget::Hosts(parse_hosts(&args.host, args.port)?),
    };

    let username = non_empty(args.username.as_deref());
    let password = non_empty(args.password.as_deref());
    let credential = match (username, password) {
        (Some(username), Some(password)) => Some(
            Credential::builder()
                .username(username.to_owned())
                .password(password.to_owned())
                .source(args.auth_source.clone())
                .build(),
        ),
        (Some(_), None) => {
            log::warn!("--username given without --password: connecting without authentication");
            None
        }
        _ => None,
    };
    if credential.is_none() && matches!(target, ConnectTarget::Hosts(_)) {
        log::warn!("Using unauthenticated connection");
    }

    let node_credential = match non_empty(args.node_username.as_deref()) {
        Some(username) => Some(
            Credential::builder()
                .username(username.to_owned())
                .password(non_empty(args.node_password.as_deref()).map(str::to_owned))
                .source(
                    non_empty(args.node_auth_source.as_deref())
                        .unwrap_or(&args.auth_source)
                        .to_owned(),
                )
                .build(),
        ),
        None => {
            if non_empty(args.node_password.as_deref()).is_some() {
                log::warn!("--node-password given without --node-username: ignored");
            }
            None
        }
    };
    if node_credential.is_some() && !args.all_nodes {
        log::warn!("--node-username only applies with --all-nodes");
    }

    Ok(ConnectConfig {
        target,
        credential,
        load_balanced: args.load_balanced,
        namespace: args.namespace.trim().to_owned(),
        hide_system_ops: !args.show_system_ops,
        all_nodes: args.all_nodes,
        node_credential,
        timeout: Duration::from_secs(args.timeout),
    })
}

fn non_empty(value: Option<&str>) -> Option<&str> {
    value.filter(|v| !v.is_empty())
}

/// Parses `--host`: a comma-separated list of `host[:port]`, IPv6 addresses
/// in brackets (`[::1]:27017`), or Unix socket paths.
pub fn parse_hosts(hosts: &str, default_port: u16) -> Result<Vec<ServerAddress>, String> {
    let mut addresses = Vec::new();
    for entry in hosts.split(',').map(str::trim).filter(|e| !e.is_empty()) {
        let address = ServerAddress::parse(entry)
            .map_err(|e| format!("invalid host {entry:?}: {}", e.kind))?;
        addresses.push(match address {
            ServerAddress::Tcp { host, port } => ServerAddress::Tcp {
                host,
                port: Some(port.unwrap_or(default_port)),
            },
            other => other,
        });
    }
    if addresses.is_empty() {
        return Err("no MongoDB host given".to_owned());
    }
    Ok(addresses)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(extra: &[&str]) -> Args {
        let mut argv = vec!["close-mongo-ops-manager"];
        argv.extend_from_slice(extra);
        Args::try_parse_from(argv).unwrap()
    }

    fn tcp(host: &str, port: u16) -> ServerAddress {
        ServerAddress::Tcp {
            host: host.to_owned(),
            port: Some(port),
        }
    }

    #[test]
    fn default_host_is_the_ipv4_loopback() {
        // Read from the definition, so MONGODB_HOST doesn't interfere.
        let command = Args::command();
        let host = command
            .get_arguments()
            .find(|a| a.get_id() == "host")
            .unwrap();
        assert_eq!(host.get_default_values(), ["127.0.0.1"]);
    }

    #[test]
    fn defaults_match_python_version() {
        // Explicit values so environment variables don't interfere.
        let a = args(&["--host", "localhost", "--port", "27017"]);
        assert_eq!(a.auth_source, "admin");
        assert_eq!(a.refresh_interval, 2);
        assert_eq!(a.timeout, 5);
        assert!(!a.show_system_ops);
        assert!(!a.load_balanced);
        assert_eq!(a.log_file, PathBuf::from(LOG_FILE));
        let config = connect_config(&a).unwrap();
        assert!(config.hide_system_ops);
        assert!(!config.all_nodes);
        assert_eq!(config.timeout, Duration::from_secs(5));
    }

    #[test]
    fn host_and_port_build_a_seed_list() {
        let a = args(&["--host", "db1, db2:27018,[::1]:27019", "--port", "27020"]);
        match connect_config(&a).unwrap().target {
            ConnectTarget::Hosts(hosts) => assert_eq!(
                hosts,
                vec![tcp("db1", 27020), tcp("db2", 27018), tcp("::1", 27019)]
            ),
            other => panic!("unexpected target {other:?}"),
        }
    }

    #[test]
    fn invalid_hosts_are_rejected() {
        assert!(parse_hosts("db1:notaport", 27017).is_err());
        assert!(parse_hosts(" , ", 27017).is_err());
    }

    #[test]
    fn credentials_need_username_and_password() {
        let a = args(&[
            "--host",
            "localhost",
            "--username",
            "u@x",
            "--password",
            "p:w/d",
            "--auth-source",
            "users",
        ]);
        let credential = connect_config(&a).unwrap().credential.unwrap();
        assert_eq!(credential.username.as_deref(), Some("u@x"));
        assert_eq!(credential.password.as_deref(), Some("p:w/d"));
        assert_eq!(credential.source.as_deref(), Some("users"));

        let a = args(&["--host", "localhost", "--username", "u", "--password", ""]);
        assert!(connect_config(&a).unwrap().credential.is_none());
    }

    #[test]
    fn uri_takes_precedence_and_is_validated() {
        let a = args(&[
            "--uri",
            "mongodb://a:1,b:2/?replicaSet=rs0",
            "--host",
            "ignored",
        ]);
        assert!(matches!(
            connect_config(&a).unwrap().target,
            ConnectTarget::Uri(uri) if uri == "mongodb://a:1,b:2/?replicaSet=rs0"
        ));
        let a = args(&["--uri", "http://nope"]);
        assert!(
            connect_config(&a)
                .unwrap_err()
                .contains("invalid connection string")
        );
        // A URI passed to --host is accepted too.
        let a = args(&["--host", "mongodb+srv://cluster.example.com"]);
        assert!(matches!(
            connect_config(&a).unwrap().target,
            ConnectTarget::Uri(_)
        ));
    }

    #[test]
    fn node_credentials_default_to_main_auth_source() {
        let a = args(&[
            "--host",
            "localhost",
            "--all-nodes",
            "--auth-source",
            "admin2",
            "--node-username",
            "shard-user",
            "--node-password",
            "secret",
        ]);
        let config = connect_config(&a).unwrap();
        assert!(config.all_nodes);
        let node = config.node_credential.unwrap();
        assert_eq!(node.username.as_deref(), Some("shard-user"));
        assert_eq!(node.source.as_deref(), Some("admin2"));
    }

    #[test]
    fn options_are_passed_through() {
        let a = args(&[
            "--host",
            "localhost",
            "--namespace",
            " app.users ",
            "--show-system-ops",
            "--load-balanced",
            "--timeout",
            "9",
        ]);
        let config = connect_config(&a).unwrap();
        assert_eq!(config.namespace, "app.users");
        assert!(!config.hide_system_ops);
        assert!(config.load_balanced);
        assert_eq!(config.timeout, Duration::from_secs(9));
    }

    #[test]
    fn refresh_interval_is_clamped_with_warning() {
        assert_eq!(refresh_interval(0), 1);
        assert_eq!(refresh_interval(3), 3);
        assert_eq!(refresh_interval(60), 10);
        let a = args(&["--refresh-interval", "-3"]);
        assert_eq!(refresh_interval(a.refresh_interval), 1);
    }

    #[test]
    fn timeout_is_validated() {
        let argv = ["close-mongo-ops-manager", "--timeout", "0"];
        assert!(Args::try_parse_from(argv).is_err());
    }

    #[test]
    fn unknown_saved_theme_falls_back_to_default() {
        let dir = tempfile::tempdir().unwrap();
        let store = ConfigStore::at(dir.path().join("config.json"));
        assert_eq!(load_theme(Some(&store)).name, theme::DEFAULT_THEME);
        store.save_theme("nord").unwrap();
        assert_eq!(load_theme(Some(&store)).name, "nord");
        store.save_theme("no-such-theme").unwrap();
        assert_eq!(load_theme(Some(&store)).name, theme::DEFAULT_THEME);
        assert_eq!(load_theme(None).name, theme::DEFAULT_THEME);
    }
}
