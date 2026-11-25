//! Strict `MAJOR.MINOR.PATCH` versions (semver without prereleases).
//!
//! Release tags are `v<Cargo version>`. A prerelease (`1.2.0-rc.1`) is never
//! offered as an update, so it does not parse; build metadata (`+abc`) is
//! ignored, as semver says.

use std::fmt;

/// A release version. Ordering is semver precedence.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Version {
    /// Incompatible changes.
    pub major: u64,
    /// Features.
    pub minor: u64,
    /// Fixes.
    pub patch: u64,
}

impl Version {
    /// A version from its parts.
    pub const fn new(major: u64, minor: u64, patch: u64) -> Self {
        Self {
            major,
            minor,
            patch,
        }
    }

    /// Parses `1.2.3` or a tag `v1.2.3`. `None` for anything else, including
    /// prereleases and leading zeros.
    pub fn parse(s: &str) -> Option<Self> {
        let s = s.trim();
        let s = s.strip_prefix('v').unwrap_or(s);
        // Build metadata does not take part in precedence.
        let core = match s.split_once('+') {
            Some((core, meta)) if valid_ident_list(meta) => core,
            Some(_) => return None,
            None => s,
        };
        if core.contains('-') {
            return None;
        }
        let mut parts = core.split('.');
        let major = number(parts.next()?)?;
        let minor = number(parts.next()?)?;
        let patch = number(parts.next()?)?;
        if parts.next().is_some() {
            return None;
        }
        Some(Self::new(major, minor, patch))
    }

    /// The running build (`CARGO_PKG_VERSION`).
    pub fn current() -> Self {
        Self::parse(env!("CARGO_PKG_VERSION")).unwrap_or(Self::new(0, 0, 0))
    }
}

fn number(s: &str) -> Option<u64> {
    if s.is_empty() || !s.bytes().all(|b| b.is_ascii_digit()) || (s.len() > 1 && s.starts_with('0'))
    {
        return None;
    }
    s.parse().ok()
}

fn valid_ident_list(s: &str) -> bool {
    !s.is_empty()
        && s.split('.')
            .all(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-'))
}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)
    }
}

impl serde::Serialize for Version {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.collect_str(self)
    }
}

impl<'de> serde::Deserialize<'de> for Version {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        Self::parse(&s).ok_or_else(|| serde::de::Error::custom("not a release version"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_tags_and_plain_versions() {
        assert_eq!(Version::parse("v0.11.0"), Some(Version::new(0, 11, 0)));
        assert_eq!(Version::parse("1.2.3"), Some(Version::new(1, 2, 3)));
        assert_eq!(Version::parse(" 10.0.20 "), Some(Version::new(10, 0, 20)));
        assert_eq!(Version::parse("1.2.3+build.5"), Some(Version::new(1, 2, 3)));
    }

    #[test]
    fn rejects_prereleases_and_junk() {
        for bad in [
            "",
            "v",
            "1",
            "1.2",
            "1.2.3.4",
            "1.2.3-rc.1",
            "v1.2.3-beta",
            "01.2.3",
            "1.02.3",
            "1.2.x",
            "1.2.3+",
            "1.2.3+a..b",
            "latest",
            "-1.2.3",
            "vv1.2.3",
        ] {
            assert_eq!(Version::parse(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn compares_numerically() {
        let v = |s| Version::parse(s).unwrap();
        assert!(v("0.11.0") > v("0.10.9"));
        assert!(v("0.10.10") > v("0.10.9"));
        assert!(v("1.0.0") > v("0.99.99"));
        assert!(v("0.11.1") > v("0.11.0"));
        assert_eq!(v("v0.11.0"), v("0.11.0+x"));
    }

    #[test]
    fn current_build_parses() {
        assert_eq!(
            Version::current().to_string(),
            env!("CARGO_PKG_VERSION").split('+').next().unwrap()
        );
    }

    #[test]
    fn serde_round_trip() {
        let v = Version::new(3, 4, 5);
        let j = serde_json::to_string(&v).unwrap();
        assert_eq!(j, "\"3.4.5\"");
        assert_eq!(serde_json::from_str::<Version>(&j).unwrap(), v);
        assert!(serde_json::from_str::<Version>("\"3.4\"").is_err());
    }
}
