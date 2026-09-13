//! Finding S2 Connect endpoints on the local network, and being found.
//!
//! `S2C §DNS-SD based discovery`: endpoints advertise `_s2connect._tcp`, with the subtype
//! `_cem` or `_rm` for each role they host, and a TXT record that carries the URLs a
//! pairing client needs. That is all discovery does — it produces a *candidate*, and
//! nothing about a candidate is trustworthy until the pairing HMAC has spoken.
//!
//! ```text
//!   _s2connect._tcp.local.                      the service
//!   _rm._sub._s2connect._tcp.local.             hosts a Resource Manager
//!   TXT  txtver=1                               this version of the specification
//!        e_name=EVSE1038                        what to show the user
//!        pairingUrl=https://EVSE1038.local/v1/  where to pair
//! ```
//!
//! The record model, including which TXT keys are mandatory and why both spellings of the
//! version key are accepted, is [`proto::dnssd`](crate::connect::proto::dnssd); this
//! module is the daemon that puts it on the wire.
//!
//! # Pure Rust, and why that matters here
//!
//! `mdns-sd` speaks mDNS itself rather than binding to Avahi or Bonjour. The alternative
//! makes a Resource Manager's firmware depend on a system daemon that constrained Linux
//! images routinely do not ship — and S2 Connect explicitly designs for nodes that are
//! "purely an HTTPS client".

use alloc::string::{String, ToString};
use alloc::vec::Vec;

use crate::connect::proto::Role;
use crate::connect::proto::dnssd::ServiceRecord;

/// The fully-qualified service type, as DNS-SD writes it.
pub const SERVICE_FQDN: &str = "_s2connect._tcp.local.";

/// An endpoint someone is advertising.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Discovered {
    /// The instance name, which is what a user would recognise in a list.
    pub instance: String,
    /// The host it answers on, without the trailing dot.
    pub host: String,
    /// The port.
    pub port: u16,
    /// Every address it resolved to.
    pub addresses: Vec<core::net::IpAddr>,
    /// What the TXT record said.
    pub record: ServiceRecord,
}

impl Discovered {
    /// The base URL of the pairing API, if this endpoint offers one.
    ///
    /// Taken from the TXT record rather than assembled from the host and port: the
    /// specification carries `pairingUrl` precisely so that an endpoint can serve the API
    /// from a path, a different port, or a different name than the one it advertises.
    #[must_use]
    pub fn pairing_url(&self) -> Option<&str> {
        self.record.pairing_url.as_deref()
    }

    /// The base URL of the long-polling API, for an endpoint that runs no HTTPS server.
    #[must_use]
    pub fn longpolling_url(&self) -> Option<&str> {
        self.record.longpolling_url.as_deref()
    }

    /// Whether this endpoint hosts a node of the given role.
    ///
    /// `None` when the subtype was not visible — some resolvers do not report it — in
    /// which case the only way to know is to ask `GET /v1/nodes`.
    #[must_use]
    pub fn hosts(&self, role: Role) -> Option<bool> {
        if self.record.roles.is_empty() {
            None
        } else {
            Some(self.record.roles.contains(&role))
        }
    }
}

/// Errors from the mDNS daemon.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// The daemon could not be started, or a record could not be registered.
    #[error("mDNS: {0}")]
    Mdns(String),
    /// The record is not one S2 Connect allows — no URL, or a version we do not speak.
    #[error("the service record is not a usable S2 Connect record")]
    Record,
}

impl From<mdns_sd::Error> for Error {
    fn from(e: mdns_sd::Error) -> Self {
        Error::Mdns(e.to_string())
    }
}

/// Advertises this endpoint, and browses for others.
///
/// One daemon per process is plenty; it is cheap to clone and each clone shares the same
/// socket.
#[derive(Clone)]
pub struct Discovery {
    daemon: mdns_sd::ServiceDaemon,
}

impl core::fmt::Debug for Discovery {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str("Discovery { .. }")
    }
}

impl Discovery {
    /// Start the daemon.
    pub fn new() -> Result<Self, Error> {
        Ok(Self {
            daemon: mdns_sd::ServiceDaemon::new()?,
        })
    }

    /// Advertise an endpoint.
    ///
    /// `instance` is the name a user sees; `host` is the `.local` name this machine
    /// answers on, without the trailing dot. The record must carry at least one of
    /// `pairingUrl` and `longpollingUrl`, or there is nothing for a client to do with it.
    ///
    /// ```no_run
    /// use s2_kit::connect::discovery::Discovery;
    /// use s2_kit::connect::proto::Role;
    /// use s2_kit::connect::proto::dnssd::ServiceRecord;
    ///
    /// # fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// let discovery = Discovery::new()?;
    /// discovery.advertise(
    ///     "EVSE1038",
    ///     "EVSE1038.local",
    ///     443,
    ///     &ServiceRecord {
    ///         endpoint_name: Some("Acme charger".into()),
    ///         pairing_url: Some("https://EVSE1038.local/v1/".into()),
    ///         roles: vec![Role::Rm],
    ///         ..ServiceRecord::default()
    ///     },
    /// )?;
    /// # Ok(())
    /// # }
    /// ```
    pub fn advertise(
        &self,
        instance: &str,
        host: &str,
        port: u16,
        record: &ServiceRecord,
    ) -> Result<(), Error> {
        // "It is mandatory to provide a value for at least one of the properties
        // pairingUrl and longpollingUrl." Advertising without one wastes every browser's
        // time and produces a candidate nothing can act on.
        if record.pairing_url.is_none() && record.longpolling_url.is_none() {
            return Err(Error::Record);
        }
        let properties: std::collections::HashMap<String, String> = record
            .txt_pairs()
            .into_iter()
            .map(|(k, v)| (k.to_string(), v))
            .collect();

        // `S2C §DNS-SD based discovery`: "`_cem` and `_rm` are both used when the endpoint
        // contains both CEM and RM nodes". `mdns-sd` cannot express that — a `ServiceInfo`
        // holds one `Option<String>` of subtype, and the daemon keys registrations by
        // *fullname*, which carries no subtype, so two registrations of one instance
        // collide rather than adding a second subtype.
        //
        // A dual-role endpoint is therefore advertised on the **plain** service type: it
        // stays discoverable by an unfiltered browse, its TXT record is unchanged, and
        // `Discovered::hosts` answers `None` for "the subtype was not visible, ask
        // `GET /v1/nodes`". Losing the filter beats losing half the audience (D44).
        let subtypes = record.subtypes();
        let ty = match subtypes.as_slice() {
            [] => SERVICE_FQDN.to_string(),
            [only] => alloc::format!("{only}._sub.{SERVICE_FQDN}"),
            _ => {
                crate::trace::event!(
                    warn,
                    instance = instance,
                    "an endpoint hosting both roles is advertised on the plain service \
                     type: mdns-sd cannot put one instance under two DNS-SD subtypes"
                );
                SERVICE_FQDN.to_string()
            }
        };

        let info = mdns_sd::ServiceInfo::new(
            &ty,
            instance,
            &alloc::format!("{}.", host.trim_end_matches('.')),
            "",
            port,
            properties,
        )?
        // Let the daemon fill in this machine's addresses; hard-coding them is how a
        // device ends up advertising an address it lost at the last DHCP lease.
        .enable_addr_auto();
        self.daemon.register(info)?;
        Ok(())
    }

    /// Stop advertising an instance.
    pub fn withdraw(&self, instance: &str) -> Result<(), Error> {
        self.daemon
            .unregister(&alloc::format!("{instance}.{SERVICE_FQDN}"))?;
        Ok(())
    }

    /// Browse for endpoints, optionally only those hosting a particular role.
    ///
    /// Returns a channel of [`Discovered`]. Records that are not usable S2 Connect
    /// records — a version we do not speak, no URL at all — are dropped rather than
    /// surfaced, because there is nothing a caller could do with them.
    ///
    /// Browsing never ends on its own: a device that appears ten minutes from now is as
    /// interesting as one that is there already. Drop the receiver to stop.
    pub fn browse(&self, role: Option<Role>) -> Result<Browse, Error> {
        let ty = match role {
            Some(role) => alloc::format!("{}._sub.{SERVICE_FQDN}", role.service_subtype()),
            None => SERVICE_FQDN.to_string(),
        };
        Ok(Browse {
            events: self.daemon.browse(&ty)?,
        })
    }

    /// Shut the daemon down, withdrawing everything it advertises.
    pub fn shutdown(&self) -> Result<(), Error> {
        self.daemon.shutdown()?;
        Ok(())
    }

    /// The underlying daemon, for anything this wrapper does not expose.
    #[must_use]
    pub fn daemon(&self) -> &mdns_sd::ServiceDaemon {
        &self.daemon
    }
}

/// A running browse.
pub struct Browse {
    events: mdns_sd::Receiver<mdns_sd::ServiceEvent>,
}

impl core::fmt::Debug for Browse {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str("Browse { .. }")
    }
}

impl Browse {
    /// The next endpoint, waiting for one if necessary.
    ///
    /// `None` when the daemon has shut down.
    pub async fn next(&self) -> Option<Discovered> {
        loop {
            let event = self.events.recv_async().await.ok()?;
            if let Some(found) = resolve(event) {
                return Some(found);
            }
        }
    }

    /// The next endpoint, or `None` if none is waiting right now.
    pub fn try_next(&self) -> Option<Discovered> {
        loop {
            let event = self.events.try_recv().ok()?;
            if let Some(found) = resolve(event) {
                return Some(found);
            }
        }
    }
}

/// Turn a resolution into a candidate, or discard it.
fn resolve(event: mdns_sd::ServiceEvent) -> Option<Discovered> {
    let mdns_sd::ServiceEvent::ServiceResolved(service) = event else {
        // `ServiceFound` has no TXT record yet and `ServiceRemoved` is not a candidate.
        return None;
    };
    let pairs: Vec<(String, String)> = service
        .txt_properties
        .iter()
        .map(|p| (p.key().to_string(), p.val_str().to_string()))
        .collect();
    let borrowed: Vec<(&str, &str)> = pairs
        .iter()
        .map(|(k, v)| (k.as_str(), v.as_str()))
        .collect();
    // Refuses a record from a future version of the specification, and one with no URL.
    let mut record = ServiceRecord::from_txt(borrowed)?;

    // The subtype, when the resolver reported one, says which role is hosted.
    if let Some(sub) = &service.sub_ty_domain {
        if sub.starts_with("_cem.") {
            record.roles = alloc::vec![Role::Cem];
        } else if sub.starts_with("_rm.") {
            record.roles = alloc::vec![Role::Rm];
        }
    }

    Some(Discovered {
        instance: service
            .fullname
            .split_once('.')
            .map_or_else(|| service.fullname.clone(), |(name, _)| name.to_string()),
        host: service.host.trim_end_matches('.').to_string(),
        port: service.port,
        addresses: service
            .addresses
            .iter()
            .map(mdns_sd::ScopedIp::to_ip_addr)
            .collect(),
        record,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn two_subtypes_cannot_share_one_instance_and_that_is_why_we_do_not_try() {
        // `S2C §DNS-SD based discovery` requires a dual-role endpoint to advertise both
        // `_cem` and `_rm`. `mdns-sd` cannot: its registrations are keyed by *fullname*,
        // and a fullname is `<instance>.<service>.<domain>` with no subtype in it. So
        // registering one instance twice does not advertise two subtypes — the second
        // silently replaces the first, and the endpoint is discoverable under whichever
        // role came last.
        //
        // This is asserted against the library rather than remembered in a comment,
        // because the day `mdns-sd` grows multi-subtype support is the day this test
        // fails and the workaround in `advertise` can go.
        let of = |sub: &str| {
            mdns_sd::ServiceInfo::new(
                &alloc::format!("{sub}._sub.{SERVICE_FQDN}"),
                "EVSE1038",
                "EVSE1038.local.",
                "127.0.0.1",
                443,
                None,
            )
            .expect("a legal service info")
        };
        let cem = of("_cem");
        let rm = of("_rm");
        assert_ne!(
            cem.get_subtype(),
            rm.get_subtype(),
            "two different subtypes"
        );
        assert_eq!(
            cem.get_fullname(),
            rm.get_fullname(),
            "…that collide on the key the daemon stores them under"
        );
    }

    fn record() -> ServiceRecord {
        ServiceRecord {
            endpoint_name: Some("Acme charger".into()),
            endpoint_logo_url: None,
            pairing_url: Some("https://EVSE1038.local/v1/".into()),
            longpolling_url: None,
            roles: alloc::vec![Role::Rm],
        }
    }

    #[test]
    fn the_service_type_is_the_one_the_specification_registers() {
        use crate::connect::proto::dnssd::{PROTOCOL, SERVICE_TYPE};
        assert_eq!(
            SERVICE_FQDN,
            alloc::format!("{SERVICE_TYPE}.{PROTOCOL}.local.")
        );
        assert_eq!(Role::Cem.service_subtype(), "_cem");
        assert_eq!(Role::Rm.service_subtype(), "_rm");
    }

    #[test]
    fn a_record_with_no_url_is_refused_before_it_reaches_the_network() {
        let Ok(discovery) = Discovery::new() else {
            // No multicast in this environment; the check below does not need a daemon.
            return;
        };
        let useless = ServiceRecord {
            roles: alloc::vec![Role::Rm],
            ..ServiceRecord::default()
        };
        assert!(matches!(
            discovery.advertise("EVSE1038", "EVSE1038.local", 443, &useless),
            Err(Error::Record)
        ));
        let _ = discovery.shutdown();
    }

    #[test]
    fn a_discovered_endpoint_reports_what_it_can_and_admits_what_it_cannot() {
        let found = Discovered {
            instance: "EVSE1038".into(),
            host: "EVSE1038.local".into(),
            port: 443,
            addresses: Vec::new(),
            record: record(),
        };
        assert_eq!(found.pairing_url(), Some("https://EVSE1038.local/v1/"));
        assert_eq!(found.longpolling_url(), None);
        assert_eq!(found.hosts(Role::Rm), Some(true));
        assert_eq!(found.hosts(Role::Cem), Some(false));

        // A resolver that did not report the subtype leaves the role unknown, and saying
        // "no" would be a lie that hides the endpoint from a browse.
        let unknown = Discovered {
            record: ServiceRecord {
                roles: Vec::new(),
                ..record()
            },
            ..found
        };
        assert_eq!(unknown.hosts(Role::Rm), None);
    }

    #[test]
    fn the_txt_record_carries_what_a_client_needs() {
        let pairs = record().txt_pairs();
        assert_eq!(pairs[0], ("txtver", "1".to_string()));
        let keys: Vec<&str> = pairs.iter().map(|(k, _)| *k).collect();
        assert!(keys.contains(&"pairingUrl"));
        assert!(keys.contains(&"e_name"));
    }
}
