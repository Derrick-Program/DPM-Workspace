use crate::{ClientError, ClientResult};
use dpm_core::CoreError;
use pubgrub::{Ranges, SemanticVersion};
use semver::{Comparator, Op, VersionReq};

/// Parses a package's own recorded version string (e.g. `"1.2.3"`) into a
/// `pubgrub::SemanticVersion`. Pre-release/build metadata (`"1.2.3-beta"`,
/// `"1.2.3+build"`) is rejected — dpm has no defined pre-release ordering
/// convention, and silently truncating to the numeric triple would make
/// `"1.0.0-alpha"` sort equal to `"1.0.0"`, which is wrong.
pub fn parse_version(s: &str) -> ClientResult<SemanticVersion> {
    let v = semver::Version::parse(s.trim()).map_err(|e| {
        ClientError::Core(CoreError::DependencyError(format!(
            "invalid version '{s}': {e}"
        )))
    })?;
    if !v.pre.is_empty() || !v.build.is_empty() {
        return Err(ClientError::Core(CoreError::DependencyError(format!(
            "version '{s}' has pre-release/build metadata, which dpm does not support"
        ))));
    }
    let triple = |n: u64, field: &str| -> ClientResult<u32> {
        u32::try_from(n).map_err(|_| {
            ClientError::Core(CoreError::DependencyError(format!(
                "version '{s}' has a {field} component that overflows u32"
            )))
        })
    };
    Ok(SemanticVersion::new(
        triple(v.major, "major")?,
        triple(v.minor, "minor")?,
        triple(v.patch, "patch")?,
    ))
}

/// Parses a dependency constraint string (e.g. `"^1.2.0"`, `"~1.2"`, `"1.2.3"`,
/// `">=1.0.0, <2.0.0"`, `""`/`"*"` for "any version") into a `pubgrub::Ranges`.
///
/// Built on `semver::VersionReq`'s parser (Cargo's own caret/tilde rules)
/// rather than a hand-rolled parser, then each comparator is converted to an
/// interval and all comparators are intersected (matching `VersionReq`'s own
/// "all comparators must hold" semantics for a comma-separated list).
pub fn parse_constraint(s: &str) -> ClientResult<Ranges<SemanticVersion>> {
    let s = s.trim();
    if s.is_empty() || s == "*" {
        return Ok(Ranges::full());
    }
    let req = VersionReq::parse(s).map_err(|e| {
        ClientError::Core(CoreError::DependencyError(format!(
            "invalid version constraint '{s}': {e}"
        )))
    })?;
    let mut result = Ranges::full();
    for comparator in &req.comparators {
        result = result.intersection(&comparator_to_range(comparator, s)?);
    }
    Ok(result)
}

fn comparator_to_range(c: &Comparator, original: &str) -> ClientResult<Ranges<SemanticVersion>> {
    let overflow = |field: &str| {
        ClientError::Core(CoreError::DependencyError(format!(
            "constraint '{original}' has a {field} component that overflows u32"
        )))
    };
    let major = u32::try_from(c.major).map_err(|_| overflow("major"))?;
    let minor = c
        .minor
        .map(u32::try_from)
        .transpose()
        .map_err(|_| overflow("minor"))?;
    let patch = c
        .patch
        .map(u32::try_from)
        .transpose()
        .map_err(|_| overflow("patch"))?;

    // A version with a missing minor/patch component denotes a *prefix*
    // range (e.g. "1" means "any 1.x.y", "1.2" means "any 1.2.z") for
    // Exact/Wildcard; Tilde/Caret give missing components their own
    // (different) meaning handled in their own arms below.
    let prefix_range = |major: u32, minor: Option<u32>| -> Ranges<SemanticVersion> {
        match minor {
            None => Ranges::between(
                SemanticVersion::new(major, 0, 0),
                SemanticVersion::new(major + 1, 0, 0),
            ),
            Some(minor) => Ranges::between(
                SemanticVersion::new(major, minor, 0),
                SemanticVersion::new(major, minor + 1, 0),
            ),
        }
    };

    Ok(match c.op {
        Op::Exact | Op::Wildcard => match (minor, patch) {
            (Some(minor), Some(patch)) => {
                Ranges::singleton(SemanticVersion::new(major, minor, patch))
            }
            _ => prefix_range(major, minor),
        },
        Op::Greater => Ranges::strictly_higher_than(SemanticVersion::new(
            major,
            minor.unwrap_or(0),
            patch.unwrap_or(0),
        )),
        Op::GreaterEq => Ranges::higher_than(SemanticVersion::new(
            major,
            minor.unwrap_or(0),
            patch.unwrap_or(0),
        )),
        Op::Less => Ranges::strictly_lower_than(SemanticVersion::new(
            major,
            minor.unwrap_or(0),
            patch.unwrap_or(0),
        )),
        Op::LessEq => Ranges::lower_than(SemanticVersion::new(
            major,
            minor.unwrap_or(0),
            patch.unwrap_or(0),
        )),
        // Tilde: allow patch-level changes if minor is given, otherwise
        // minor-level changes (Cargo's own tilde rule).
        Op::Tilde => match minor {
            Some(minor) => Ranges::between(
                SemanticVersion::new(major, minor, patch.unwrap_or(0)),
                SemanticVersion::new(major, minor + 1, 0),
            ),
            None => prefix_range(major, None),
        },
        // Caret: Cargo's own rule — bump at the leftmost non-zero component.
        Op::Caret => {
            let minor = minor.unwrap_or(0);
            let patch = patch.unwrap_or(0);
            let lower = SemanticVersion::new(major, minor, patch);
            let upper = if major > 0 {
                SemanticVersion::new(major + 1, 0, 0)
            } else if minor > 0 {
                SemanticVersion::new(0, minor + 1, 0)
            } else {
                SemanticVersion::new(0, 0, patch + 1)
            };
            Ranges::between(lower, upper)
        }
        _ => {
            return Err(ClientError::Core(CoreError::DependencyError(format!(
                "constraint '{original}' uses an unsupported comparator"
            ))))
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_plain_triple() {
        let v = parse_version("1.2.3").unwrap();
        assert_eq!(v, SemanticVersion::new(1, 2, 3));
    }

    #[test]
    fn rejects_pre_release() {
        assert!(parse_version("1.2.3-beta.1").is_err());
    }

    #[test]
    fn rejects_garbage() {
        assert!(parse_version("not-a-version").is_err());
    }

    fn v(major: u32, minor: u32, patch: u32) -> SemanticVersion {
        SemanticVersion::new(major, minor, patch)
    }

    #[test]
    fn empty_and_star_mean_any_version() {
        let full = Ranges::full();
        assert_eq!(parse_constraint("").unwrap(), full);
        assert_eq!(parse_constraint("*").unwrap(), full);
    }

    #[test]
    fn exact_pin() {
        let r = parse_constraint("1.2.3").unwrap();
        assert!(r.contains(&v(1, 2, 3)));
        assert!(!r.contains(&v(1, 2, 4)));
        assert!(!r.contains(&v(1, 2, 2)));
    }

    #[test]
    fn caret_normal() {
        let r = parse_constraint("^1.2.3").unwrap();
        assert!(r.contains(&v(1, 2, 3)));
        assert!(r.contains(&v(1, 9, 0)));
        assert!(!r.contains(&v(2, 0, 0)));
        assert!(!r.contains(&v(1, 2, 2)));
    }

    #[test]
    fn caret_zero_major() {
        // ^0.2.3 := >=0.2.3, <0.3.0
        let r = parse_constraint("^0.2.3").unwrap();
        assert!(r.contains(&v(0, 2, 3)));
        assert!(!r.contains(&v(0, 3, 0)));
    }

    #[test]
    fn caret_zero_zero() {
        // ^0.0.3 := >=0.0.3, <0.0.4
        let r = parse_constraint("^0.0.3").unwrap();
        assert!(r.contains(&v(0, 0, 3)));
        assert!(!r.contains(&v(0, 0, 4)));
    }

    #[test]
    fn tilde() {
        // ~1.2.3 := >=1.2.3, <1.3.0
        let r = parse_constraint("~1.2.3").unwrap();
        assert!(r.contains(&v(1, 2, 9)));
        assert!(!r.contains(&v(1, 3, 0)));
    }

    #[test]
    fn comparator_range() {
        let r = parse_constraint(">=1.0.0, <2.0.0").unwrap();
        assert!(r.contains(&v(1, 5, 0)));
        assert!(!r.contains(&v(2, 0, 0)));
        assert!(!r.contains(&v(0, 9, 0)));
    }

    #[test]
    fn invalid_constraint_is_an_error() {
        assert!(parse_constraint("not a constraint").is_err());
    }
}
