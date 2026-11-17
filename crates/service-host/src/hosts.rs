//! The manifest's host entries (SERVICE-PROTOCOL.md §1, "Host entries"): a
//! lower-case DNS name, or one behind `*.`, and the check that a wildcard
//! does not cover a public suffix.
//!
//! The Public Suffix List is a dated copy carried in this crate,
//! `data/public_suffix_list.dat`, as publicsuffix.org serves it (its header
//! names its version and commit), the ICANN and the private sections alike:
//! `*.co.uk`, `*.github.io` and `*.workers.dev` would admit every customer of
//! a registry or a hosting platform. It changes with this repository, never
//! at run time.

use std::collections::HashSet;
use std::sync::OnceLock;

/// The list, as carried.
const LIST: &str = include_str!("../data/public_suffix_list.dat");

/// At most this many bytes in a name (DNS's own bound).
const MAX_NAME_BYTES: usize = 253;

/// The version line of the carried list (`2026-11-17_06-40-19_UTC`).
pub fn list_version() -> &'static str {
    LIST.lines().find_map(|line| line.strip_prefix("// VERSION: ")).unwrap_or("unknown")
}

/// Whether `name` is a host name as an entry writes it: lower-case labels of
/// `[a-z0-9-]`, none starting or ending with `-`, at least two of them, at
/// most 253 bytes. A name whose last label is all digits is an address
/// written as a name (`192.0.2.1`), which no entry may be.
pub fn valid_name(name: &str) -> bool {
    if name.is_empty() || name.len() > MAX_NAME_BYTES {
        return false;
    }
    let labels: Vec<&str> = name.split('.').collect();
    let label_ok = |label: &str| {
        !label.is_empty()
            && label.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
            && !label.starts_with('-')
            && !label.ends_with('-')
    };
    labels.len() >= 2 && labels.iter().all(|label| label_ok(label)) && !labels.last().is_some_and(|tld| tld.bytes().all(|b| b.is_ascii_digit()))
}

/// What is wrong with one host entry, or None for a good one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EntryFault {
    /// Not a name, nor `*.` and a name.
    Syntax,
    /// `*.` and a public suffix.
    PublicSuffix(String),
}

/// Checks one entry: `name.tld`, or `*.name.tld` whose `name.tld` is not a
/// public suffix.
pub fn check_entry(entry: &str) -> Option<EntryFault> {
    match entry.strip_prefix("*.") {
        Some(base) if valid_name(base) => is_public_suffix(base).then(|| EntryFault::PublicSuffix(base.to_owned())),
        Some(_) => Some(EntryFault::Syntax),
        None if valid_name(entry) => None,
        None => Some(EntryFault::Syntax),
    }
}

struct Rules {
    normal: HashSet<String>,
    /// `*.x`, by `x`.
    wildcard: HashSet<String>,
    /// `!x`, by `x`.
    exception: HashSet<String>,
}

fn rules() -> &'static Rules {
    static RULES: OnceLock<Rules> = OnceLock::new();
    RULES.get_or_init(|| {
        let mut rules = Rules { normal: HashSet::new(), wildcard: HashSet::new(), exception: HashSet::new() };
        for line in LIST.lines() {
            let Some(rule) = line.split_whitespace().next() else { continue };
            if rule.starts_with("//") {
                continue;
            }
            let (set, name) = if let Some(name) = rule.strip_prefix('!') {
                (&mut rules.exception, name)
            } else if let Some(name) = rule.strip_prefix("*.") {
                (&mut rules.wildcard, name)
            } else {
                (&mut rules.normal, rule)
            };
            // Rules are written in Unicode; names are compared as ASCII, as
            // a URL's host is after IDNA conversion.
            if let Ok(ascii) = idna::domain_to_ascii(name) {
                set.insert(ascii);
            }
        }
        rules
    })
}

/// Whether `name` (ASCII, lower-case) is itself a public suffix, by the
/// list's algorithm (publicsuffix.org/list, "Formal Algorithm"): the
/// matching exception rule prevails, else the matching rule with the most
/// labels, else the implicit `*`; the name is a public suffix when that rule
/// covers all of it.
pub fn is_public_suffix(name: &str) -> bool {
    let rules = rules();
    let labels: Vec<&str> = name.split('.').collect();
    let n = labels.len();
    let mut longest = 1;
    let mut exception = None;
    for k in 1..=n {
        let suffix = labels[n - k..].join(".");
        if rules.exception.contains(&suffix) {
            exception = Some(k);
        }
        if rules.normal.contains(&suffix) {
            longest = longest.max(k);
        }
        if k >= 2 && rules.wildcard.contains(&labels[n - k + 1..].join(".")) {
            longest = longest.max(k);
        }
    }
    let public = match exception {
        Some(k) => k - 1,
        None => longest,
    };
    public == n
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_as_entries_write_them() {
        for good in ["x.com", "api.x.com", "a-b.example", "xn--fiqs8s.example", "3com.com"] {
            assert!(valid_name(good), "{good}");
        }
        for bad in ["", "com", "X.com", "-a.com", "a-.com", "a..com", "a.com.", ".a.com", "a_b.com", "192.0.2.1", "a.com:80", "http://a.com", "*.a.com"] {
            assert!(!valid_name(bad), "{bad}");
        }
        assert!(valid_name(&format!("{}.com", "a".repeat(249))));
        assert!(!valid_name(&format!("{}.com", "a".repeat(250))));
    }

    #[test]
    fn a_wildcard_may_not_cover_a_public_suffix() {
        assert!(list_version().starts_with("20"), "{}", list_version());
        for public in ["com", "co.uk", "github.io", "workers.dev", "ck", "xn--fiqs8s"] {
            assert!(is_public_suffix(public), "{public}");
        }
        // `*.ck` makes every name under ck a public suffix, `!www.ck` excepted.
        assert!(is_public_suffix("anything.ck"));
        assert!(!is_public_suffix("www.ck"));
        for private in ["googlevideo.com", "x.com", "bbc.co.uk", "pocket-nexus.github.io", "example.workers.dev", "nolisted.zz"] {
            assert!(!is_public_suffix(private), "{private}");
        }
        assert_eq!(check_entry("*.googlevideo.com"), None);
        assert_eq!(check_entry("api.x.com"), None);
        assert_eq!(check_entry("*.co.uk"), Some(EntryFault::PublicSuffix("co.uk".into())));
        assert_eq!(check_entry("*.github.io"), Some(EntryFault::PublicSuffix("github.io".into())));
        assert_eq!(check_entry("*.com"), Some(EntryFault::Syntax), "a wildcard needs a name of two labels");
        assert_eq!(check_entry("*.*.x.com"), Some(EntryFault::Syntax));
        assert_eq!(check_entry("a.*.x.com"), Some(EntryFault::Syntax));
    }
}
