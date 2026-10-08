//! Client options of the main connection and of direct connections to
//! cluster members.

use std::time::Duration;

use mongodb::Client;
use mongodb::options::{
    ClientOptions, Compressor, ConnectionString, Credential, HostInfo, ReadPreference,
    SelectionCriteria, ServerAddress, ServerApi, Tls,
};

use super::call::{bounded, describe_error};
use super::topology::{format_address, seeds_display};
use super::{APP_NAME, ConnectConfig, ConnectTarget};
use crate::error::MongoOpsError;

/// Options of the main connection, ready to connect.
#[derive(Debug, Clone)]
pub(crate) struct MainOptions {
    pub options: ClientOptions,
    /// Display name of the connection target, without credentials.
    pub target: String,
    pub load_balanced: bool,
}

/// The error for a connection string that cannot be parsed. The parser's
/// message is not used: it can quote parts of the connection string,
/// including an unencoded password.
pub(crate) const INVALID_CONNECTION_STRING: &str =
    "invalid connection string (percent-encode special characters in the username and password)";

/// Parses the connection target and applies our overrides.
pub(crate) async fn main_options(config: &ConnectConfig) -> Result<MainOptions, MongoOpsError> {
    let (mut options, target) = match &config.target {
        ConnectTarget::Uri(uri) => {
            let connection_string = ConnectionString::parse(uri)
                .map_err(|_| MongoOpsError::Connection(INVALID_CONNECTION_STRING.to_owned()))?;
            let target = match &connection_string.host_info {
                HostInfo::DnsRecord(name) => name.clone(),
                HostInfo::HostIdentifiers(hosts) => seeds_display(hosts),
                _ => String::from("(unknown)"),
            };
            let options = bounded(
                "resolving the connection string",
                config.timeout,
                ClientOptions::parse(connection_string),
            )
            .await
            .map_err(|e| e.into_connection_error())?;
            (options, target)
        }
        ConnectTarget::Hosts(hosts) => {
            if hosts.is_empty() {
                return Err(MongoOpsError::Connection("no host to connect to".into()));
            }
            let mut options = ClientOptions::default();
            options.hosts.clone_from(hosts);
            (options, seeds_display(hosts))
        }
    };
    apply_overrides(&mut options, config);
    Ok(MainOptions {
        load_balanced: options.load_balanced == Some(true),
        target,
        options,
    })
}

/// Our defaults for whatever the connection string did not set, and the
/// options we always force.
pub(crate) fn apply_overrides(options: &mut ClientOptions, config: &ConnectConfig) {
    let uri_credential = options.credential.take();
    let (credential, conflict) = merge_credential(
        uri_credential,
        config.credential.clone(),
        options.default_database.as_deref(),
    );
    if conflict {
        log::warn!("The connection string has credentials; ignoring the username/password options");
    }
    options.credential = credential;

    // Our operations are hidden by our application name: never another
    // application's, which would hide all of its operations.
    if let Some(app_name) = options.app_name.as_deref().filter(|n| *n != APP_NAME) {
        log::debug!("Using the application name {APP_NAME} instead of {app_name}");
    }
    options.app_name = Some(APP_NAME.to_owned());
    options
        .server_selection_timeout
        .get_or_insert(config.timeout);
    options.connect_timeout.get_or_insert(config.timeout);
    options.compressors.get_or_insert_with(default_compressors);

    // $currentOp only reports operations on the server that runs it. Through
    // mongos it fans out to the member of each shard selected by the read
    // preference: the primary shows every shard primary's operations.
    let primary = SelectionCriteria::ReadPreference(ReadPreference::Primary);
    if options
        .selection_criteria
        .as_ref()
        .is_some_and(|c| *c != primary)
    {
        log::info!(
            "Ignoring the read preference of the connection string: reading from the primary"
        );
    }
    options.selection_criteria = Some(primary);

    if config.load_balanced {
        options.load_balanced = Some(true);
    }
}

/// Credentials from the connection string win over the username/password
/// options. Returns whether both were given.
pub(crate) fn merge_credential(
    from_uri: Option<Credential>,
    from_options: Option<Credential>,
    uri_database: Option<&str>,
) -> (Option<Credential>, bool) {
    match (from_uri, from_options) {
        (Some(uri), options) => (Some(uri), options.is_some()),
        (None, Some(mut credential)) => {
            if credential.source.is_none() {
                credential.source = uri_database.map(str::to_owned);
            }
            (Some(credential), false)
        }
        (None, None) => (None, false),
    }
}

fn default_compressors() -> Vec<Compressor> {
    vec![
        Compressor::Zstd { level: None },
        Compressor::Snappy,
        Compressor::Zlib { level: None },
    ]
}

/// Builds clients connected directly to one server (cluster member or
/// mongos).
///
/// Only the options that apply to a single server are copied from the main
/// connection: its parsed options may carry SRV polling state, a replica set
/// name or load balancing, which break direct connections.
#[derive(Debug, Clone)]
pub(crate) struct DirectClients {
    /// The credential of the main connection. Through mongos it is a user
    /// stored on the config servers, valid on mongos and config servers.
    main_credential: Option<Credential>,
    /// `--node-username`: for shard members, which only know shard-local
    /// users.
    node_credential: Option<Credential>,
    tls: Option<Tls>,
    compressors: Option<Vec<Compressor>>,
    connect_timeout: Option<Duration>,
    server_selection_timeout: Option<Duration>,
    server_api: Option<ServerApi>,
}

/// Connections per member: a listing, a `hello` and a kill at most.
const DIRECT_POOL_SIZE: u32 = 4;

/// Which credential a direct connection uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum Users {
    /// The main credential: for mongos, config servers, and the servers of
    /// a replica set or standalone main connection.
    Cluster,
    /// The node credential if given, else the main one: for shard and
    /// replica set members.
    Node,
}

impl DirectClients {
    pub fn new(main: &MainOptions, node_credential: Option<Credential>) -> Self {
        let options = &main.options;
        Self {
            main_credential: options.credential.clone(),
            node_credential,
            tls: options.tls.clone(),
            compressors: options.compressors.clone(),
            connect_timeout: options.connect_timeout,
            server_selection_timeout: options.server_selection_timeout,
            server_api: options.server_api.clone(),
        }
    }

    pub fn has_node_credential(&self) -> bool {
        self.node_credential.is_some()
    }

    /// Options for a direct connection to `address`. With `set_name`, the
    /// driver checks the server's replica set name (from its unauthenticated
    /// `hello`) and refuses a server of another replica set before
    /// authenticating to it.
    pub fn options(
        &self,
        address: &str,
        users: Users,
        set_name: Option<&str>,
    ) -> Result<ClientOptions, String> {
        let address = ServerAddress::parse(address)
            .map_err(|e| format!("invalid address {address}: {}", describe_error(&e)))?;
        let mut options = ClientOptions::default();
        options.hosts = vec![pin_loopback(address)];
        options.direct_connection = Some(true);
        options.repl_set_name = set_name.map(str::to_owned);
        options.credential = match users {
            Users::Cluster => self.main_credential.clone(),
            Users::Node => self
                .node_credential
                .clone()
                .or_else(|| self.main_credential.clone()),
        };
        options.tls.clone_from(&self.tls);
        options.compressors.clone_from(&self.compressors);
        options.app_name = Some(APP_NAME.to_owned());
        options.connect_timeout = self.connect_timeout;
        options.server_selection_timeout = self.server_selection_timeout;
        options.server_api.clone_from(&self.server_api);
        options.max_pool_size = Some(DIRECT_POOL_SIZE);
        options.min_pool_size = Some(0);
        Ok(options)
    }

    /// A new client for `address`. Connections are made lazily.
    pub fn client(
        &self,
        address: &str,
        users: Users,
        set_name: Option<&str>,
    ) -> Result<Client, String> {
        let options = self.options(address, users, set_name)?;
        let display = format_address(&options.hosts[0]);
        Client::with_options(options)
            .map_err(|e| format!("cannot connect to {display}: {}", describe_error(&e)))
    }
}

/// Members named `localhost` (by the cluster's configuration) are reached at
/// 127.0.0.1, where mongod listens by default. The name may resolve to ::1
/// first, which mongod does not bind by default and another local user could.
fn pin_loopback(address: ServerAddress) -> ServerAddress {
    match address {
        ServerAddress::Tcp { host, port } if host.eq_ignore_ascii_case("localhost") => {
            ServerAddress::Tcp {
                host: "127.0.0.1".to_owned(),
                port,
            }
        }
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mongodb::options::AuthMechanism;

    fn config(target: ConnectTarget) -> ConnectConfig {
        ConnectConfig {
            target,
            credential: None,
            load_balanced: false,
            namespace: String::new(),
            hide_system_ops: true,
            all_nodes: false,
            node_credential: None,
            timeout: Duration::from_secs(3),
        }
    }

    fn credential(user: &str, source: Option<&str>) -> Credential {
        let mut c = Credential::default();
        c.username = Some(user.into());
        c.password = Some("secret".into());
        c.source = source.map(str::to_owned);
        c
    }

    async fn parse(uri: &str, config_credential: Option<Credential>) -> MainOptions {
        let mut c = config(ConnectTarget::Uri(uri.into()));
        c.credential = config_credential;
        main_options(&c).await.unwrap()
    }

    #[tokio::test]
    async fn defaults_for_a_plain_uri() {
        let main = parse("mongodb://db1:27017/", None).await;
        let o = &main.options;
        assert_eq!(o.app_name.as_deref(), Some(APP_NAME));
        assert_eq!(o.server_selection_timeout, Some(Duration::from_secs(3)));
        assert_eq!(o.connect_timeout, Some(Duration::from_secs(3)));
        assert_eq!(o.compressors, Some(default_compressors()));
        assert_eq!(
            o.selection_criteria,
            Some(SelectionCriteria::ReadPreference(ReadPreference::Primary))
        );
        assert_eq!(o.load_balanced, None);
        assert_eq!(o.credential, None);
        assert_eq!(main.target, "db1:27017");
        assert!(!main.load_balanced);
    }

    #[tokio::test]
    async fn uri_settings_are_kept_except_the_read_preference_and_app_name() {
        let main = parse(
            "mongodb://db1,db2:27018/?appName=ops&serverSelectionTimeoutMS=900\
             &connectTimeoutMS=800&compressors=zlib&readPreference=secondary",
            None,
        )
        .await;
        let o = &main.options;
        // Our operations are hidden by our name, never by another app's.
        assert_eq!(o.app_name.as_deref(), Some(APP_NAME));
        assert_eq!(o.server_selection_timeout, Some(Duration::from_millis(900)));
        assert_eq!(o.connect_timeout, Some(Duration::from_millis(800)));
        assert_eq!(o.compressors, Some(vec![Compressor::Zlib { level: None }]));
        assert_eq!(
            o.selection_criteria,
            Some(SelectionCriteria::ReadPreference(ReadPreference::Primary))
        );
        assert_eq!(main.target, "db1:27017 +1");
    }

    #[tokio::test]
    async fn load_balanced_option() {
        let mut c = config(ConnectTarget::Uri("mongodb://lb:27017/".into()));
        c.load_balanced = true;
        let main = main_options(&c).await.unwrap();
        assert_eq!(main.options.load_balanced, Some(true));
        assert!(main.load_balanced);

        let main = parse("mongodb://lb:27017/?loadBalanced=true", None).await;
        assert!(main.load_balanced);
    }

    #[tokio::test]
    async fn credential_from_options_is_used_without_uri_credentials() {
        let main = parse("mongodb://db1/", Some(credential("alice", Some("admin")))).await;
        let c = main.options.credential.unwrap();
        assert_eq!(c.username.as_deref(), Some("alice"));
        assert_eq!(c.source.as_deref(), Some("admin"));

        // Without a source, the database of the connection string is used.
        let main = parse("mongodb://db1/reports", Some(credential("alice", None))).await;
        assert_eq!(
            main.options.credential.unwrap().source.as_deref(),
            Some("reports")
        );
    }

    #[tokio::test]
    async fn uri_credentials_win() {
        let main = parse(
            "mongodb://bob:pw@db1/?authSource=ops",
            Some(credential("alice", Some("admin"))),
        )
        .await;
        let c = main.options.credential.unwrap();
        assert_eq!(c.username.as_deref(), Some("bob"));
        assert_eq!(c.source.as_deref(), Some("ops"));
    }

    #[test]
    fn merge_credential_cases() {
        assert_eq!(merge_credential(None, None, Some("db")), (None, false));
        let (merged, conflict) = merge_credential(
            Some(credential("bob", None)),
            Some(credential("alice", None)),
            None,
        );
        assert_eq!(merged.unwrap().username.as_deref(), Some("bob"));
        assert!(conflict);
        let mut x509 = Credential::default();
        x509.mechanism = Some(AuthMechanism::MongoDbX509);
        let (merged, conflict) = merge_credential(Some(x509), None, None);
        assert_eq!(merged.unwrap().mechanism, Some(AuthMechanism::MongoDbX509));
        assert!(!conflict);
    }

    #[tokio::test]
    async fn host_list_target() {
        let hosts = vec![
            ServerAddress::parse("db1:27017").unwrap(),
            ServerAddress::parse("db2:27017").unwrap(),
        ];
        let mut c = config(ConnectTarget::Hosts(hosts.clone()));
        c.credential = Some(credential("alice", Some("admin")));
        let main = main_options(&c).await.unwrap();
        assert_eq!(main.options.hosts, hosts);
        assert_eq!(main.target, "db1:27017 +1");
        assert_eq!(
            main.options.credential.unwrap().username.as_deref(),
            Some("alice")
        );

        let c = config(ConnectTarget::Hosts(vec![]));
        assert!(matches!(
            main_options(&c).await,
            Err(MongoOpsError::Connection(_))
        ));
    }

    #[tokio::test]
    async fn invalid_uri_is_a_connection_error_without_details() {
        for uri in [
            "http://db1",
            "mongodb://bob:pa?ss@db1/",
            "mongodb://bob:p@ss:w0rd@db1/",
            "mongodb://bob:secret%zz@db1/",
        ] {
            let c = config(ConnectTarget::Uri(uri.into()));
            match main_options(&c).await {
                Err(MongoOpsError::Connection(message)) => {
                    assert_eq!(message, INVALID_CONNECTION_STRING, "{uri}")
                }
                other => panic!("unexpected {other:?} for {uri}"),
            }
        }
    }

    #[test]
    fn target_never_includes_credentials() {
        let hosts = ConnectionString::parse("mongodb://bob:pw@db1:27017,db2/").unwrap();
        match hosts.host_info {
            HostInfo::HostIdentifiers(h) => assert_eq!(seeds_display(&h), "db1:27017 +1"),
            other => panic!("unexpected {other:?}"),
        }
        let srv = ConnectionString::parse("mongodb+srv://bob:pw@cluster0.example.net/").unwrap();
        assert_eq!(
            srv.host_info,
            HostInfo::DnsRecord("cluster0.example.net".into())
        );
    }

    #[tokio::test]
    async fn direct_clients_copy_only_single_server_options() {
        let mut c = config(ConnectTarget::Uri(
            "mongodb://bob:pw@db1,db2/?replicaSet=rs0&tls=true&appName=ops&readPreference=secondary"
                .into(),
        ));
        c.timeout = Duration::from_secs(2);
        let main = main_options(&c).await.unwrap();
        let direct = DirectClients::new(&main, None);
        let o = direct.options("DB2:27018", Users::Node, None).unwrap();
        assert_eq!(o.hosts, vec![ServerAddress::parse("db2:27018").unwrap()]);
        assert_eq!(o.direct_connection, Some(true));
        assert_eq!(o.repl_set_name, None);
        assert_eq!(o.load_balanced, None);
        assert_eq!(o.selection_criteria, None);
        assert_eq!(o.app_name.as_deref(), Some(APP_NAME));
        assert_eq!(
            o.credential.as_ref().unwrap().username.as_deref(),
            Some("bob")
        );
        assert!(matches!(o.tls, Some(Tls::Enabled(_))));
        assert_eq!(o.compressors, Some(default_compressors()));
        assert_eq!(o.server_selection_timeout, Some(Duration::from_secs(2)));
        assert_eq!(o.connect_timeout, Some(Duration::from_secs(2)));
        assert_eq!(o.max_pool_size, Some(DIRECT_POOL_SIZE));

        let node = DirectClients::new(&main, Some(credential("shard-local", Some("admin"))));
        let o = node.options("cfg1:27019", Users::Cluster, None).unwrap();
        assert_eq!(o.credential.unwrap().username.as_deref(), Some("bob"));
        let o = node.options("db2:27018", Users::Node, None).unwrap();
        assert_eq!(
            o.credential.unwrap().username.as_deref(),
            Some("shard-local")
        );
        assert!(direct.options("db2:x", Users::Node, None).is_err());
    }

    #[tokio::test]
    async fn direct_clients_bind_the_replica_set_and_pin_localhost() {
        let c = config(ConnectTarget::Uri("mongodb://db1/".into()));
        let main = main_options(&c).await.unwrap();
        let direct = DirectClients::new(&main, None);
        let o = direct
            .options("db2:27018", Users::Node, Some("shard01"))
            .unwrap();
        assert_eq!(o.repl_set_name.as_deref(), Some("shard01"));

        let o = direct
            .options("LocalHost:37021", Users::Node, None)
            .unwrap();
        assert_eq!(
            o.hosts,
            vec![ServerAddress::parse("127.0.0.1:37021").unwrap()]
        );
        let o = direct.options("[::1]:37021", Users::Node, None).unwrap();
        assert_eq!(o.hosts, vec![ServerAddress::parse("[::1]:37021").unwrap()]);
    }
}
