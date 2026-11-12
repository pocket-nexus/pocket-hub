//! The HTTP clients requests share, and with them their idle connections.
//!
//! A client per request paid DNS, TCP and TLS on every request (on a real
//! New 3DS XL: 630 to 1,450 ms each for a few hundred bytes). Requests now
//! take their client from here and leave a finished HTTP/1.1 connection idle
//! in its pool for `IDLE` (at most `IDLE_PER_HOST` per host), where the next
//! request to the same host picks it up.
//!
//! **Which connection a request may reuse.** The HTTP library keys its pool
//! by the URL's scheme, host as written and port, and hands a request only a
//! connection of exactly that key. Hub adds the rest of the key by keeping
//! separate clients, whose pools are never mixed:
//!
//! - **Per provider.** Every `NetProvider` (with its `NetConfig`: overrides,
//!   resolver, extra roots) has its own clients; nothing is process-wide.
//! - **Per applet** (the request's `app`). A server may tie state to a
//!   connection rather than to a request: NTLM and Negotiate authenticate
//!   the connection, and a server may do the same with a session of its
//!   own. A connection one applet authenticated must not carry another
//!   applet's request, so applets do not share connections. Requests of one
//!   applet do; that is the reuse the measured case needs (one applet, one
//!   host). TLS session tickets are per client as well.
//! - **Override hosts apart from every other host.** A `ResolveOverride`'s
//!   host is reached through a client of its own whose resolver knows the
//!   overrides and nothing else; the public clients' resolver never sees an
//!   override. A connection to an override's private address therefore lives
//!   only in an override pool, and a public lookup of any name only ever
//!   produces a connection in a public pool.
//! - **Per proxy plan.** The public clients are made for one `ProxyPlan`
//!   (the settings read for the request, proxies pinned to their address).
//!   A request whose plan differs drops every public client made under the
//!   previous plan, idle connections with them (a request still running on
//!   one keeps its own handle until it ends, then that connection closes
//!   too). Under one plan a URL always takes the same route, direct or
//!   through the same proxy, so a pool key never mixes routes.
//!
//! **Why reuse cannot reach an address the policy refuses today.**
//! - Direct: a pooled connection was opened to an address `CheckedResolver`
//!   returned for exactly the host of its key, after every address of that
//!   answer was classified public (`fetch.rs`). Classification depends on
//!   the address alone, so that address would pass today. A request reuses
//!   it only for the same scheme, host name and port: a name that resolves
//!   to a private address now never rides a connection made for another
//!   name, and its own new connections go through the resolver and are
//!   refused. A name whose answer changed keeps, for at most `IDLE` after
//!   its last use, the public address it was checked at. A literal IP host
//!   is classified by `check_hop` before every hop, pooled or not.
//! - Behind a proxy: Hub's own lookup of the destination runs before every
//!   hop whether or not a pooled connection exists (`check_hop`); a refused
//!   name ends the request before the HTTP library is asked. A pooled tunnel
//!   was opened right after such a check passed for the same name, so it
//!   goes where the proxy resolved that name then; the gap `proxy.rs`
//!   describes is the same gap, not a wider one.
//! - Override hosts are the owner's own setting, fixed for the provider's
//!   life; their pool holds connections to the override's address only.
//!
//! **What a pooled connection never carries over.** No cookie store (the
//! HTTP library's `cookies` feature is off) and no default headers: every
//! header of a request is the request's own (`fetch.rs`). An HTTP/1.1
//! connection returns to the pool only once its response was read to the
//! end. A request that stops early (a device `CANCEL`, the link ending, its
//! own timeout, `maxBytes` reached, a body error) drops its response; the
//! library then makes one read of what has already arrived, without waiting,
//! and pools the connection only if that read ends the response exactly
//! (its length or its last chunk); otherwise it closes the connection at
//! once. The next request therefore gets a connection at a message boundary
//! or a new one, never the middle of an earlier response. HTTP/2 is
//! not offered (`http1_only`): its default windows would let a server send
//! megabytes per stream ahead of the device's credit, and the test server
//! here could not show cancellation and backpressure on it.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use reqwest::dns::{Addrs, Name, Resolve, Resolving};

use crate::proxy::ProxyPlan;
use crate::{Lookup, NetConfig, NetError, ResolveOverride, codes};

/// How long a connection waits idle in a pool for the next request.
pub(crate) const IDLE: Duration = Duration::from_secs(90);
/// The idle connections a pool keeps per host: the 8 open requests of a
/// device rarely all go to one host at once.
pub(crate) const IDLE_PER_HOST: usize = 4;

/// One client and when a request last took it. A client nobody took for
/// `IDLE` has no idle connection left and is dropped from the table, so the
/// table holds only the applets of the last 90 s however many a peer names.
struct Entry {
    client: reqwest::Client,
    used: Instant,
}

#[derive(Default)]
struct Table {
    /// The plan the public clients were made under.
    plan: Option<ProxyPlan>,
    /// By applet: destinations other than the overrides.
    public: HashMap<String, Entry>,
    /// By applet: the override hosts.
    overrides: HashMap<String, Entry>,
}

/// A provider's clients (module comment).
#[derive(Default)]
pub(crate) struct Clients {
    table: Mutex<Table>,
}

fn take(map: &mut HashMap<String, Entry>, app: &str, make: impl FnOnce() -> Result<reqwest::Client, NetError>) -> Result<reqwest::Client, NetError> {
    let now = Instant::now();
    map.retain(|_, entry| now.duration_since(entry.used) < IDLE);
    if let Some(entry) = map.get_mut(app) {
        entry.used = now;
        return Ok(entry.client.clone());
    }
    let client = make()?;
    map.insert(app.to_owned(), Entry { client: client.clone(), used: now });
    Ok(client)
}

impl Clients {
    /// The client of `app` for destinations other than the overrides, under `plan`.
    pub(crate) fn public(&self, config: &NetConfig, app: &str, plan: &ProxyPlan) -> Result<reqwest::Client, NetError> {
        let mut table = self.table.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        if table.plan.as_ref() != Some(plan) {
            table.public.clear();
            table.plan = Some(plan.clone());
        }
        let plan = Arc::new(plan.clone());
        take(&mut table.public, app, || public_client(config, plan))
    }

    /// The client of `app` for the override hosts.
    pub(crate) fn overrides(&self, config: &NetConfig, app: &str) -> Result<reqwest::Client, NetError> {
        let mut table = self.table.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        take(&mut table.overrides, app, || override_client(config))
    }
}

/// What every client shares: no redirects (Hub follows them by hand, each
/// hop checked), HTTP/1.1 only, the pool's bounds, no proxy but the plan's.
fn builder(config: &NetConfig) -> Result<reqwest::ClientBuilder, NetError> {
    let mut builder = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .http1_only()
        .pool_idle_timeout(IDLE)
        .pool_max_idle_per_host(IDLE_PER_HOST)
        .tcp_nodelay(true)
        .no_proxy();
    for der in &config.extra_roots {
        let certificate = reqwest::Certificate::from_der(der).map_err(|error| NetError::new(codes::OTHER, format!("an extra root certificate does not parse: {error}")))?;
        builder = builder.tls_certs_merge([certificate]);
    }
    Ok(builder)
}

fn build(builder: reqwest::ClientBuilder) -> Result<reqwest::Client, NetError> {
    builder.build().map_err(|error| NetError::new(codes::OTHER, format!("the HTTP client cannot start: {}", crate::fetch::chain_text(&error))))
}

fn public_client(config: &NetConfig, plan: Arc<ProxyPlan>) -> Result<reqwest::Client, NetError> {
    build(builder(config)?.dns_resolver(CheckedResolver { lookup: config.lookup.clone() }).proxy(reqwest::Proxy::custom(move |url| plan.for_url(url).cloned())))
}

/// An override's host goes to the owner's server directly, never through a proxy.
fn override_client(config: &NetConfig) -> Result<reqwest::Client, NetError> {
    build(builder(config)?.dns_resolver(OverrideResolver { overrides: Arc::new(config.overrides.clone()) }))
}

/// The resolver of the public clients' direct path: every name looked up
/// and refused when any address is not public (`fetch.rs`).
struct CheckedResolver {
    lookup: Arc<dyn Lookup>,
}

impl Resolve for CheckedResolver {
    fn resolve(&self, name: Name) -> Resolving {
        let host = name.as_str().to_owned();
        let lookup = self.lookup.clone();
        Box::pin(async move {
            match crate::fetch::resolve_checked(&*lookup, &host).await {
                Ok(found) => Ok(Box::new(found.into_iter()) as Addrs),
                Err(error) => Err(Box::new(error) as Box<dyn std::error::Error + Send + Sync>),
            }
        })
    }
}

/// The resolver of the override clients: an override's address as given,
/// and no other name. `fetch::open` sends only override hosts here, so a
/// name it does not know is Hub's own mistake, refused rather than looked up.
struct OverrideResolver {
    overrides: Arc<Vec<ResolveOverride>>,
}

impl Resolve for OverrideResolver {
    fn resolve(&self, name: Name) -> Resolving {
        let host = name.as_str().to_owned();
        let found = self.overrides.iter().find(|o| o.host.eq_ignore_ascii_case(&host)).map(|o| o.address);
        Box::pin(async move {
            match found {
                Some(address) => Ok(Box::new(std::iter::once(address)) as Addrs),
                None => Err(Box::new(NetError::new(codes::OTHER, format!("{host} reached the override client and is not an override"))) as Box<dyn std::error::Error + Send + Sync>),
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Proxies, ProxySettings};

    fn plan(no_proxy: &str) -> ProxyPlan {
        ProxySettings { http: Some("127.0.0.1:3128".into()), https: None, no_proxy: no_proxy.into() }.plan().unwrap()
    }

    fn count(clients: &Clients) -> (usize, usize) {
        let table = clients.table.lock().unwrap();
        (table.public.len(), table.overrides.len())
    }

    #[test]
    fn one_client_per_applet_and_plan_and_none_kept_across_a_plan_change() {
        let config = NetConfig { proxies: Proxies::Direct, ..NetConfig::default() };
        let clients = Clients::default();
        clients.public(&config, "a", &plan("")).unwrap();
        clients.public(&config, "a", &plan("")).unwrap();
        clients.public(&config, "b", &plan("")).unwrap();
        clients.overrides(&config, "a").unwrap();
        assert_eq!(count(&clients), (2, 1));
        // Another plan: the public clients of the previous plan are gone,
        // the override clients (never behind a proxy) stay.
        clients.public(&config, "a", &plan("x.test")).unwrap();
        assert_eq!(count(&clients), (1, 1));
        assert_eq!(clients.table.lock().unwrap().plan, Some(plan("x.test")));
    }

    #[test]
    fn clients_unused_for_the_idle_time_leave_the_table() {
        let config = NetConfig { proxies: Proxies::Direct, ..NetConfig::default() };
        let clients = Clients::default();
        clients.public(&config, "old", &plan("")).unwrap();
        clients.table.lock().unwrap().public.get_mut("old").unwrap().used -= IDLE;
        clients.public(&config, "new", &plan("")).unwrap();
        let table = clients.table.lock().unwrap();
        assert!(table.public.contains_key("new") && !table.public.contains_key("old"));
    }
}
