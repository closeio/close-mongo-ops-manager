//! Deployment and member identification from `hello` responses, and member
//! address handling.

use mongodb::bson::{Bson, Document};
use mongodb::options::ServerAddress;

use crate::model::{Deployment, NodeRole};

/// The fields of a `hello` (or legacy `isMaster`) response we use.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Hello {
    /// `msg: "isdbgrid"`: a mongos.
    pub is_mongos: bool,
    pub set_name: Option<String>,
    /// `isWritablePrimary` (`ismaster` on old servers).
    pub writable_primary: bool,
    pub secondary: bool,
    pub arbiter_only: bool,
    /// `isreplicaset`: a replica set member without a configuration yet.
    pub is_replica_set: bool,
    pub hosts: Vec<String>,
    pub passives: Vec<String>,
    pub arbiters: Vec<String>,
    pub me: Option<String>,
    /// `serviceId`: the connection goes through a load balancer.
    pub service_id: bool,
}

impl Hello {
    pub fn parse(doc: &Document) -> Self {
        let flag = |key: &str| matches!(doc.get(key), Some(Bson::Boolean(true)));
        let string = |key: &str| match doc.get(key) {
            Some(Bson::String(s)) if !s.is_empty() => Some(s.clone()),
            _ => None,
        };
        let list = |key: &str| -> Vec<String> {
            doc.get_array(key)
                .map(|items| {
                    items
                        .iter()
                        .filter_map(|i| i.as_str())
                        .filter(|s| !s.is_empty())
                        .map(str::to_owned)
                        .collect()
                })
                .unwrap_or_default()
        };
        Self {
            is_mongos: string("msg").as_deref() == Some("isdbgrid"),
            set_name: string("setName"),
            writable_primary: flag("isWritablePrimary") || flag("ismaster"),
            secondary: flag("secondary"),
            arbiter_only: flag("arbiterOnly"),
            is_replica_set: flag("isreplicaset"),
            hosts: list("hosts"),
            passives: list("passives"),
            arbiters: list("arbiters"),
            me: string("me"),
            service_id: doc.contains_key("serviceId"),
        }
    }

    /// Kind of deployment behind a connection whose server answered this.
    pub fn deployment(&self) -> Deployment {
        if self.is_mongos {
            Deployment::Sharded
        } else if let Some(name) = &self.set_name {
            Deployment::ReplicaSet { name: name.clone() }
        } else {
            Deployment::Standalone
        }
    }

    /// Role of the member that answered this.
    pub fn role(&self) -> NodeRole {
        if self.is_mongos {
            NodeRole::Mongos
        } else if self.arbiter_only {
            NodeRole::Arbiter
        } else if self.set_name.is_none() && !self.is_replica_set {
            if self.writable_primary {
                NodeRole::Standalone
            } else {
                NodeRole::Other
            }
        } else if self.writable_primary {
            NodeRole::Primary
        } else if self.secondary {
            NodeRole::Secondary
        } else {
            NodeRole::Other
        }
    }

    /// Data-bearing members reported by a replica set member (`hosts` and
    /// `passives`; arbiters and hidden members are not included).
    pub fn data_members(&self) -> impl Iterator<Item = &str> {
        self.hosts
            .iter()
            .chain(self.passives.iter())
            .map(String::as_str)
    }
}

/// Splits a replica set connection string as found in `listShards`
/// (`"rs0/h1:27017,h2:27017"`) or `configsvrConnectionString`, or a bare host
/// list (`"h1:27017"`), into the set name and the hosts.
pub(crate) fn parse_replica_set_hosts(value: &str) -> (Option<String>, Vec<String>) {
    let value = value.trim();
    let (name, hosts) = match value.split_once('/') {
        Some((name, hosts)) => {
            let name = name.trim();
            ((!name.is_empty()).then(|| name.to_owned()), hosts)
        }
        None => (None, value),
    };
    let hosts = hosts
        .split(',')
        .map(str::trim)
        .filter(|h| !h.is_empty())
        .map(str::to_owned)
        .collect();
    (name, hosts)
}

/// `host:port` of an address, with IPv6 hosts in brackets and the default
/// port made explicit.
pub(crate) fn format_address(address: &ServerAddress) -> String {
    match address {
        ServerAddress::Tcp { host, port } => {
            let port = port.unwrap_or(27017);
            if host.contains(':') {
                format!("[{host}]:{port}")
            } else {
                format!("{host}:{port}")
            }
        }
        other => other.to_string(),
    }
}

/// Canonical form of a member address (lowercase host, explicit port), used
/// to compare addresses reported by different servers. `None` if invalid.
pub(crate) fn normalize_address(address: &str) -> Option<String> {
    ServerAddress::parse(address.trim())
        .ok()
        .map(|a| format_address(&a))
}

/// The name a server gives itself, from its `hostInfo` response
/// (`system.hostname`): the `host` of the operations it reports in
/// `$currentOp`, normalized.
pub(crate) fn host_info_name(response: &Document) -> Option<String> {
    let name = response
        .get_document("system")
        .ok()?
        .get_str("hostname")
        .ok()?;
    normalize_address(name)
}

/// Display name of a seed list: the first host, plus how many more.
pub(crate) fn seeds_display(hosts: &[ServerAddress]) -> String {
    match hosts {
        [] => "(no hosts)".to_owned(),
        [first] => format_address(first),
        [first, rest @ ..] => format!("{} +{}", format_address(first), rest.len()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mongodb::bson::doc;

    #[test]
    fn mongos_hello() {
        let hello = Hello::parse(&doc! {
            "isWritablePrimary": true,
            "msg": "isdbgrid",
            "maxWireVersion": 25,
            "ok": 1,
        });
        assert!(hello.is_mongos);
        assert_eq!(hello.deployment(), Deployment::Sharded);
        assert_eq!(hello.role(), NodeRole::Mongos);
        assert!(!hello.service_id);
    }

    #[test]
    fn load_balanced_mongos_hello() {
        let hello = Hello::parse(&doc! {
            "isWritablePrimary": true,
            "msg": "isdbgrid",
            "serviceId": mongodb::bson::oid::ObjectId::new(),
        });
        assert!(hello.service_id);
        assert_eq!(hello.deployment(), Deployment::Sharded);
    }

    #[test]
    fn replica_set_primary_hello() {
        let hello = Hello::parse(&doc! {
            "hosts": ["localhost:37024"],
            "passives": ["localhost:37025"],
            "setName": "shard02",
            "isWritablePrimary": true,
            "secondary": false,
            "primary": "localhost:37024",
            "me": "localhost:37024",
        });
        assert_eq!(
            hello.deployment(),
            Deployment::ReplicaSet {
                name: "shard02".into()
            }
        );
        assert_eq!(hello.role(), NodeRole::Primary);
        assert_eq!(hello.me.as_deref(), Some("localhost:37024"));
        assert_eq!(
            hello.data_members().collect::<Vec<_>>(),
            ["localhost:37024", "localhost:37025"]
        );
    }

    #[test]
    fn secondary_arbiter_and_other_roles() {
        let secondary = Hello::parse(&doc! {
            "setName": "rs0", "isWritablePrimary": false, "secondary": true,
        });
        assert_eq!(secondary.role(), NodeRole::Secondary);

        let arbiter = Hello::parse(&doc! {
            "hosts": ["a:1", "b:2"],
            "arbiters": ["c:3"],
            "setName": "rs0",
            "isWritablePrimary": false,
            "secondary": false,
            "arbiterOnly": true,
        });
        assert_eq!(arbiter.role(), NodeRole::Arbiter);
        assert_eq!(arbiter.arbiters, ["c:3"]);

        let recovering = Hello::parse(&doc! {
            "setName": "rs0", "isWritablePrimary": false, "secondary": false,
        });
        assert_eq!(recovering.role(), NodeRole::Other);

        let uninitialized = Hello::parse(&doc! {
            "isWritablePrimary": false, "secondary": false, "isreplicaset": true,
        });
        assert_eq!(uninitialized.role(), NodeRole::Other);
        assert_eq!(uninitialized.deployment(), Deployment::Standalone);
    }

    #[test]
    fn standalone_and_legacy_hello() {
        let standalone = Hello::parse(&doc! { "isWritablePrimary": true, "ok": 1.0 });
        assert_eq!(standalone.deployment(), Deployment::Standalone);
        assert_eq!(standalone.role(), NodeRole::Standalone);

        let legacy = Hello::parse(&doc! { "ismaster": true, "setName": "rs0" });
        assert_eq!(legacy.role(), NodeRole::Primary);
    }

    #[test]
    fn replica_set_host_strings() {
        assert_eq!(
            parse_replica_set_hosts("shard01/h1:1,h2:2"),
            (Some("shard01".into()), vec!["h1:1".into(), "h2:2".into()])
        );
        assert_eq!(
            parse_replica_set_hosts(" h1:27017 "),
            (None, vec!["h1:27017".into()])
        );
        assert_eq!(
            parse_replica_set_hosts("h1:1, h2:2,"),
            (None, vec!["h1:1".into(), "h2:2".into()])
        );
        assert_eq!(
            parse_replica_set_hosts("configRS/[::1]:27019,[fe80::1]:27019"),
            (
                Some("configRS".into()),
                vec!["[::1]:27019".into(), "[fe80::1]:27019".into()]
            )
        );
        assert_eq!(
            parse_replica_set_hosts("rs0/"),
            (Some("rs0".into()), vec![])
        );
        assert_eq!(parse_replica_set_hosts(""), (None, vec![]));
    }

    #[test]
    fn address_normalization() {
        assert_eq!(
            normalize_address("DB1.Example.com:27018").as_deref(),
            Some("db1.example.com:27018")
        );
        assert_eq!(normalize_address("db1").as_deref(), Some("db1:27017"));
        assert_eq!(
            normalize_address("[::1]:27017").as_deref(),
            Some("[::1]:27017")
        );
        assert_eq!(normalize_address("[::1]").as_deref(), Some("[::1]:27017"));
        assert_eq!(normalize_address("db1:notaport"), None);
        assert_eq!(normalize_address(""), None);
    }

    #[test]
    fn host_info_names() {
        let response = mongodb::bson::doc! {
            "system": { "hostname": "27d265b2ec2a", "numCores": 8 },
            "ok": 1,
        };
        // The default port is implied.
        assert_eq!(
            host_info_name(&response).as_deref(),
            Some("27d265b2ec2a:27017")
        );
        let response = mongodb::bson::doc! { "system": { "hostname": "Db1:37030" } };
        assert_eq!(host_info_name(&response).as_deref(), Some("db1:37030"));
        assert_eq!(host_info_name(&mongodb::bson::doc! { "ok": 1 }), None);
    }

    #[test]
    fn seed_list_display() {
        let a = ServerAddress::parse("db1:27017").unwrap();
        let b = ServerAddress::parse("db2:27017").unwrap();
        let c = ServerAddress::parse("[::1]:27018").unwrap();
        assert_eq!(seeds_display(std::slice::from_ref(&a)), "db1:27017");
        assert_eq!(seeds_display(&[a, b, c.clone()]), "db1:27017 +2");
        assert_eq!(seeds_display(&[c]), "[::1]:27018");
        assert_eq!(seeds_display(&[]), "(no hosts)");
        let default_port = ServerAddress::Tcp {
            host: "db3".into(),
            port: None,
        };
        assert_eq!(seeds_display(&[default_port]), "db3:27017");
    }
}
