//! Proxies for the requests Hub performs (§6.5): `HTTPS_PROXY`,
//! `HTTP_PROXY` and `NO_PROXY` from Hub's environment, and the system's
//! settings when none of the proxy variables is set, as the reference
//! (`fetch.ts` `adoptSystemProxy`) does.
//!
//! **What the destination check means behind a proxy.** The proxy, not Hub,
//! opens the connection to the destination, so Hub cannot check the address
//! that connection uses. Hub resolves the destination's name itself and
//! refuses the request when any address is not public, then hands the name
//! to the proxy, which resolves it again: a name whose answer changes
//! between the two lookups, or that the proxy resolves differently (its own
//! DNS, its own view of an internal network), reaches what the proxy
//! reaches. That is the reference's check (`checkDestination` before
//! `fetch`); a literal IP is checked as it is, with no gap. The proxy
//! itself is the owner's setting and may be on a private address. Hub
//! resolves a proxy's name at each request and connects to that address,
//! so no destination lookup goes unchecked on the direct path; a proxy whose
//! address changed makes another plan, and another set of connections
//! (`clients.rs`). Connection reuse behind a proxy: see `clients.rs`.

use std::net::IpAddr;
use std::sync::Arc;

use reqwest::Url;

use crate::{NetError, codes};

/// Where the proxy settings come from.
#[derive(Clone)]
pub enum Proxies {
    /// No proxy.
    Direct,
    /// These settings.
    Fixed(ProxySettings),
    /// The system's settings, read at each request so a change (a VPN or a
    /// proxy tool switched on) applies to the next request. On macOS: the
    /// HTTP and HTTPS proxies of the current network settings and their
    /// exceptions (`scutil --proxy` shows them). Elsewhere there are none.
    System,
    /// Settings read from this function at each request, as `System` reads
    /// the system's: a test changes the settings between requests with it.
    Read(Arc<dyn Fn() -> ProxySettings + Send + Sync>),
}

impl std::fmt::Debug for Proxies {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Proxies::Direct => f.write_str("Direct"),
            Proxies::Fixed(settings) => f.debug_tuple("Fixed").field(settings).finish(),
            Proxies::System => f.write_str("System"),
            Proxies::Read(_) => f.write_str("Read(..)"),
        }
    }
}

/// One set of proxy settings, in the environment variables' forms.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProxySettings {
    /// The proxy for `http:` URLs: `http://host:port` or `host:port`.
    pub http: Option<String>,
    /// The proxy for `https:` URLs (an HTTP proxy that tunnels with `CONNECT`).
    pub https: Option<String>,
    /// curl's `NO_PROXY` rules: comma-separated names (a name matches itself
    /// and its subdomains; a leading `.` or `*.` is ignored), IP addresses
    /// and CIDR blocks; `*` matches every host.
    pub no_proxy: String,
}

impl Proxies {
    /// Hub's default: the environment's variables when one of
    /// `HTTPS_PROXY`, `https_proxy`, `HTTP_PROXY`, `http_proxy` is set, else
    /// the system's settings.
    pub fn from_environment() -> Proxies {
        let var = |names: &[&str]| names.iter().find_map(|n| std::env::var(n).ok().filter(|v| !v.trim().is_empty()));
        let http = var(&["HTTP_PROXY", "http_proxy"]);
        let https = var(&["HTTPS_PROXY", "https_proxy"]);
        if http.is_none() && https.is_none() {
            return Proxies::System;
        }
        Proxies::Fixed(ProxySettings { http, https, no_proxy: var(&["NO_PROXY", "no_proxy"]).unwrap_or_default() })
    }

    /// The settings in force now.
    pub(crate) fn current(&self) -> Result<ProxySettings, NetError> {
        match self {
            Proxies::Direct => Ok(ProxySettings::default()),
            Proxies::Fixed(settings) => Ok(settings.clone()),
            Proxies::System => system_settings(),
            Proxies::Read(read) => Ok(read()),
        }
    }
}

/// The proxy settings of one request, parsed. Two plans are equal when they
/// send every URL the same way, to the same proxy address: the shared
/// clients (`clients.rs`) are kept for one plan and dropped when it changes.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct ProxyPlan {
    pub http: Option<Url>,
    pub https: Option<Url>,
    no: Vec<NoProxy>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum NoProxy {
    All,
    Domain(String),
    Address(IpAddr),
    Block(IpAddr, u8),
}

fn proxy_url(value: &str) -> Result<Url, NetError> {
    let text = if value.contains("://") { value.to_owned() } else { format!("http://{value}") };
    let url = Url::parse(&text).map_err(|error| NetError::new(codes::OTHER, format!("the proxy setting {value} does not parse: {error}")))?;
    if url.scheme() != "http" {
        return Err(NetError::new(codes::OTHER, format!("the proxy {} uses {}:, and Hub speaks to HTTP proxies only", url.host_str().unwrap_or(value), url.scheme())));
    }
    if url.host_str().is_none_or(str::is_empty) {
        return Err(NetError::new(codes::OTHER, format!("the proxy setting {value} names no host")));
    }
    Ok(url)
}

fn no_proxy_rule(entry: &str) -> Option<NoProxy> {
    let entry = entry.trim();
    if entry.is_empty() {
        return None;
    }
    if entry == "*" {
        return Some(NoProxy::All);
    }
    if let Some((address, bits)) = entry.split_once('/')
        && let (Ok(address), Ok(bits)) = (address.parse::<IpAddr>(), bits.parse::<u8>())
    {
        return Some(NoProxy::Block(address, bits));
    }
    let bare = entry.trim_start_matches('[').trim_end_matches(']');
    if let Ok(address) = bare.parse::<IpAddr>() {
        return Some(NoProxy::Address(address));
    }
    Some(NoProxy::Domain(entry.trim_start_matches("*.").trim_start_matches('.').to_ascii_lowercase()))
}

fn in_block(address: IpAddr, base: IpAddr, bits: u8) -> bool {
    match (address, base) {
        (IpAddr::V4(a), IpAddr::V4(b)) if bits <= 32 => {
            let mask = if bits == 0 { 0 } else { u32::MAX << (32 - bits as u32) };
            u32::from(a) & mask == u32::from(b) & mask
        }
        (IpAddr::V6(a), IpAddr::V6(b)) if bits <= 128 => {
            let mask = if bits == 0 { 0 } else { u128::MAX << (128 - bits as u32) };
            u128::from(a) & mask == u128::from(b) & mask
        }
        _ => false,
    }
}

impl ProxySettings {
    pub(crate) fn plan(&self) -> Result<ProxyPlan, NetError> {
        Ok(ProxyPlan {
            http: self.http.as_deref().map(proxy_url).transpose()?,
            https: self.https.as_deref().map(proxy_url).transpose()?,
            no: self.no_proxy.split(',').filter_map(no_proxy_rule).collect(),
        })
    }
}

impl ProxyPlan {
    /// The proxy a URL goes through, or None for a direct connection.
    pub fn for_url(&self, url: &Url) -> Option<&Url> {
        let proxy = match url.scheme() {
            "http" => self.http.as_ref(),
            "https" => self.https.as_ref(),
            _ => None,
        }?;
        let host = url.host_str()?.trim_start_matches('[').trim_end_matches(']').to_ascii_lowercase();
        let address = host.parse::<IpAddr>().ok();
        let excluded = self.no.iter().any(|rule| match rule {
            NoProxy::All => true,
            NoProxy::Domain(domain) => host == *domain || host.ends_with(&format!(".{domain}")),
            NoProxy::Address(a) => address == Some(*a),
            NoProxy::Block(base, bits) => address.is_some_and(|a| in_block(a, *base, *bits)),
        });
        if excluded { None } else { Some(proxy) }
    }

    /// Every proxy of the plan, for resolving their names once.
    pub fn proxies_mut(&mut self) -> impl Iterator<Item = &mut Url> {
        self.http.iter_mut().chain(self.https.iter_mut())
    }
}

#[cfg(target_os = "macos")]
fn system_settings() -> Result<ProxySettings, NetError> {
    use system_configuration::core_foundation::array::CFArray;
    use system_configuration::core_foundation::base::{CFType, TCFType};
    use system_configuration::core_foundation::number::CFNumber;
    use system_configuration::core_foundation::string::CFString;
    use system_configuration::dynamic_store::SCDynamicStoreBuilder;

    let unreadable = || NetError::new(codes::OTHER, "the macOS proxy settings cannot be read (SCDynamicStoreCopyProxies failed)");
    let store = SCDynamicStoreBuilder::new("pocket-hub").build().ok_or_else(unreadable)?;
    let proxies = store.get_proxies().ok_or_else(unreadable)?;
    let find = |key: &'static str| proxies.find(CFString::from_static_string(key)).map(|value| value.clone());
    let number = |key: &'static str| find(key).and_then(|v| v.downcast::<CFNumber>()).and_then(|n| n.to_i64());
    let text = |key: &'static str| find(key).and_then(|v| v.downcast::<CFString>()).map(|s| s.to_string());
    // The reference's reading (`adoptSystemProxy`): a proxy is in use when
    // its `Enable` is 1 and it names a host; a missing port is the scheme's.
    let proxy = |enable: &'static str, host: &'static str, port: &'static str, default: i64| -> Option<String> {
        if number(enable) != Some(1) {
            return None;
        }
        let host = text(host).filter(|h| !h.is_empty())?;
        Some(format!("http://{host}:{}", number(port).unwrap_or(default)))
    };
    let https = proxy("HTTPSEnable", "HTTPSProxy", "HTTPSPort", 443);
    let http = proxy("HTTPEnable", "HTTPProxy", "HTTPPort", 80);
    if https.is_none() && http.is_none() {
        return Ok(ProxySettings::default());
    }
    let mut no: Vec<String> = vec!["localhost".into(), "127.0.0.1".into(), "::1".into()];
    if let Some(list) = find("ExceptionsList").and_then(|v| v.downcast::<CFArray>()) {
        for raw in list.iter() {
            // SAFETY: the array's items are CF objects the array retains while
            // `list` lives; the get rule takes a reference of its own.
            let item = unsafe { CFType::wrap_under_get_rule(*raw) };
            if let Some(entry) = item.downcast::<CFString>().map(|s| s.to_string())
                && !entry.starts_with('<')
            {
                no.push(entry);
            }
        }
    }
    Ok(ProxySettings { http, https, no_proxy: no.join(",") })
}

#[cfg(not(target_os = "macos"))]
fn system_settings() -> Result<ProxySettings, NetError> {
    // Only macOS has system proxy settings Hub reads; elsewhere the
    // environment's variables are the settings, and none are set.
    Ok(ProxySettings::default())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plan(http: Option<&str>, https: Option<&str>, no: &str) -> ProxyPlan {
        ProxySettings { http: http.map(Into::into), https: https.map(Into::into), no_proxy: no.into() }.plan().unwrap()
    }

    #[test]
    fn schemes_and_no_proxy_rules() {
        let p = plan(Some("proxy.lan:3128"), Some("http://user:pw@10.0.0.2:8080"), "example.org, .internal.test,*.corp, 93.184.0.0/16, 2001:4860::/32, 1.1.1.1");
        let via = |u: &str| p.for_url(&Url::parse(u).unwrap()).map(|x| x.to_string());
        assert_eq!(via("http://a.example.com/").as_deref(), Some("http://proxy.lan:3128/"));
        assert_eq!(via("https://a.example.com/").as_deref(), Some("http://user:pw@10.0.0.2:8080/"));
        assert_eq!(via("https://example.org/"), None);
        assert_eq!(via("https://www.example.org/"), None);
        assert!(via("https://notexample.org/").is_some());
        assert_eq!(via("https://x.internal.test/"), None);
        assert_eq!(via("https://a.corp/"), None);
        assert_eq!(via("https://93.184.215.14/"), None);
        assert!(via("https://93.185.0.1/").is_some());
        assert_eq!(via("https://[2001:4860:4860::8888]/"), None);
        assert_eq!(via("https://1.1.1.1/"), None);
        assert_eq!(plan(Some("p:1"), Some("p:1"), "*").for_url(&Url::parse("https://a.b/").unwrap()), None);
        assert!(plan(None, Some("p:1"), "").for_url(&Url::parse("http://a.b/").unwrap()).is_none());
    }

    #[test]
    fn only_http_proxies() {
        for value in ["socks5://127.0.0.1:1080", "https://proxy:443"] {
            let error = ProxySettings { https: Some(value.into()), ..Default::default() }.plan().unwrap_err();
            assert_eq!(error.code, "other", "{value}");
        }
    }
}
