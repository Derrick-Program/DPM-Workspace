use crate::{ClientError, ClientResult, DbPackage};
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
///
/// One deliberate deviation from raw `VersionReq` semantics: `VersionReq`
/// treats a bare, unprefixed version (`"1.2.3"`) as `^1.2.3` (Cargo.toml's
/// own convention — indistinguishable from an explicit `^` once parsed,
/// since both produce `Op::Caret`). dpm instead follows the `pkg@version`
/// CLI convention (as in `npm install pkg@1.2.3`): a bare digits-and-dots
/// string pins exactly that version (or prefix-range for a partial version
/// like `"1.2"`, meaning "any 1.2.x"). This is done by rewriting a bare
/// version to an explicit `=`-prefixed one before parsing, so it reuses the
/// existing `Op::Exact` handling in `comparator_to_range` rather than a
/// separate code path.
pub fn parse_constraint(s: &str) -> ClientResult<Ranges<SemanticVersion>> {
    let original = s.trim();
    if original.is_empty() || original == "*" {
        return Ok(Ranges::full());
    }
    let rewritten;
    let s = if original.chars().all(|c| c.is_ascii_digit() || c == '.') {
        rewritten = format!("={original}");
        rewritten.as_str()
    } else {
        original
    };
    // Error messages below use `original` (what the user actually typed),
    // never the internally-rewritten `s` — e.g. a bare "1.2.3" that
    // overflows u32 should report '1.2.3', not the injected '=1.2.3'.
    let req = VersionReq::parse(s).map_err(|e| {
        ClientError::Core(CoreError::DependencyError(format!(
            "invalid version constraint '{original}': {e}"
        )))
    })?;
    let mut result = Ranges::full();
    for comparator in &req.comparators {
        result = result.intersection(&comparator_to_range(comparator, original)?);
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

/// Splits a CLI install argument into `(source_hint, name, constraint)`.
/// Syntax (npm-like): `[source/]name[@constraint]`.
///
/// Examples: `"foo"` -> `(None, "foo", None)`; `"official/foo"` ->
/// `(Some("official"), "foo", None)`; `"foo@^1.2"` -> `(None, "foo",
/// Some("^1.2"))`; `"official/foo@1.0.0"` -> `(Some("official"), "foo",
/// Some("1.0.0"))`.
pub fn parse_package_spec(spec: &str) -> (Option<&str>, &str, Option<&str>) {
    let (name_part, constraint) = match spec.split_once('@') {
        Some((n, c)) => (n, Some(c)),
        None => (spec, None),
    };
    let (source, name) = match name_part.split_once('/') {
        Some((s, n)) => (Some(s), n),
        None => (None, name_part),
    };
    (source, name, constraint)
}

/// pubgrub's package identifier: `(source, name)`, matching the same
/// collision-safety rule `install()` already uses for bare package names
/// (spec: "package identifier 用 (source, name) tuple"). A bare tuple
/// doesn't implement `Display`, which `pubgrub::Package` requires, hence
/// the newtype.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct PkgId {
    pub source: String,
    pub name: String,
}

impl std::fmt::Display for PkgId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}/{}", self.source, self.name)
    }
}

/// Synthetic package representing "everything the user asked to install in
/// this one CLI invocation" — its only role is to be the pubgrub root and
/// depend on every requested package. Not a real, installable package.
fn root_id() -> PkgId {
    PkgId {
        source: "$root".to_string(),
        name: "$root".to_string(),
    }
}

/// Applies the "0 sources -> not found, 1 -> use it, 2+ -> ambiguous" rule
/// used throughout `install()`, so a package name (whether typed on the CLI
/// or found inside another package's `dependencies` list) always resolves
/// to exactly one source or fails clearly.
fn resolve_source_for_name(
    all_packages: &[DbPackage],
    name: &str,
    source_hint: Option<&str>,
) -> ClientResult<String> {
    if let Some(hint) = source_hint {
        return if all_packages
            .iter()
            .any(|p| p.source == hint && p.name == name)
        {
            Ok(hint.to_string())
        } else {
            Err(ClientError::Core(CoreError::PackageNotFound(format!(
                "{hint}/{name}"
            ))))
        };
    }
    let mut sources: Vec<&str> = all_packages
        .iter()
        .filter(|p| p.name == name)
        .map(|p| p.source.as_str())
        .collect();
    sources.sort_unstable();
    sources.dedup();
    match sources.len() {
        0 => Err(ClientError::Core(CoreError::PackageNotFound(
            name.to_string(),
        ))),
        1 => Ok(sources[0].to_string()),
        _ => Err(ClientError::Core(CoreError::AmbiguousPackage(format!(
            "{name} (found in: {})",
            sources.join(", ")
        )))),
    }
}

use pubgrub::{DefaultStringReporter, OfflineDependencyProvider, PubGrubError, Reporter};

type Provider = OfflineDependencyProvider<PkgId, Ranges<SemanticVersion>>;

/// Loads the entire local index (`all_packages`, from `Db::read_all()`) into
/// an in-memory pubgrub provider, adds a synthetic root depending on every
/// `requests` entry, and solves once for the whole set. Returns `(source,
/// name, version_string)` triples for every *non-root* package in the
/// solution — callers look these back up in `all_packages` to get the full
/// `DbPackage` row (kind, url/hash or build_command, entry, ...).
///
/// `requests` is `(source_hint, name, constraint_string)` per requested
/// package, e.g. from `parse_package_spec`.
pub fn resolve_install_set(
    all_packages: &[DbPackage],
    requests: &[(Option<String>, String, Option<String>)],
) -> ClientResult<Vec<(String, String, String)>> {
    let mut provider: Provider = OfflineDependencyProvider::new();

    // Every (source, name) pair gets every one of its versions registered,
    // with that version's own dependencies resolved to concrete (source,
    // name) pairs right now (not lazily during solving) — we already have
    // the full index in memory, so ambiguous/missing dependency names fail
    // fast with a clear error instead of surfacing as a confusing NoSolution.
    for pkg in all_packages {
        let pkg_id = PkgId {
            source: pkg.source.clone(),
            name: pkg.name.clone(),
        };
        let version = parse_version(&pkg.version)?;

        let mut deps: Vec<(PkgId, Ranges<SemanticVersion>)> = Vec::new();
        if let Some(dependencies) = &pkg.dependencies {
            for dep in dependencies {
                let dep_source = resolve_source_for_name(all_packages, &dep.name, None)?;
                let dep_range = parse_constraint(&dep.version)?;
                deps.push((
                    PkgId {
                        source: dep_source,
                        name: dep.name.clone(),
                    },
                    dep_range,
                ));
            }
        }
        provider.add_dependencies(pkg_id, version, deps);
    }

    let mut root_deps: Vec<(PkgId, Ranges<SemanticVersion>)> = Vec::new();
    for (source_hint, name, constraint) in requests {
        let source = resolve_source_for_name(all_packages, name, source_hint.as_deref())?;
        let range = match constraint {
            Some(c) => parse_constraint(c)?,
            None => Ranges::full(),
        };
        root_deps.push((
            PkgId {
                source,
                name: name.clone(),
            },
            range,
        ));
    }
    let root = root_id();
    provider.add_dependencies(root.clone(), SemanticVersion::new(0, 0, 0), root_deps);

    let solution = pubgrub::resolve(&provider, root.clone(), SemanticVersion::new(0, 0, 0))
        .map_err(format_pubgrub_error)?;

    Ok(solution
        .into_iter()
        .filter(|(id, _)| *id != root)
        .map(|(id, version)| (id.source, id.name, version.to_string()))
        .collect())
}

fn format_pubgrub_error(err: PubGrubError<Provider>) -> ClientError {
    match err {
        PubGrubError::NoSolution(mut tree) => {
            tree.collapse_no_versions();
            let report = DefaultStringReporter::report(&tree);
            ClientError::Core(CoreError::DependencyError(report))
        }
        other => ClientError::Core(CoreError::DependencyError(other.to_string())),
    }
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

    #[test]
    fn spec_bare_name() {
        assert_eq!(parse_package_spec("foo"), (None, "foo", None));
    }

    #[test]
    fn spec_with_source() {
        assert_eq!(
            parse_package_spec("official/foo"),
            (Some("official"), "foo", None)
        );
    }

    #[test]
    fn spec_with_constraint() {
        assert_eq!(parse_package_spec("foo@^1.2"), (None, "foo", Some("^1.2")));
    }

    #[test]
    fn spec_with_source_and_constraint() {
        assert_eq!(
            parse_package_spec("official/foo@1.0.0"),
            (Some("official"), "foo", Some("1.0.0"))
        );
    }

    use dpm_core::Dependency;

    fn pkg(source: &str, name: &str, version: &str, deps: Vec<(&str, &str)>) -> DbPackage {
        let dependencies = if deps.is_empty() {
            None
        } else {
            Some(
                deps.into_iter()
                    .map(|(n, v)| Dependency::new(n, v))
                    .collect(),
            )
        };
        DbPackage::new(
            source,
            name,
            version,
            "prebuilt",
            Some(format!("https://example.com/{name}-{version}.zip")),
            Some("deadbeef".to_string()),
            Some(format!("{name}.zip")),
            None,
            "",
            "",
            dependencies,
        )
    }

    #[test]
    fn resolves_simple_request_to_newest_version() {
        let all = vec![
            pkg("official", "foo", "1.0.0", vec![]),
            pkg("official", "foo", "1.1.0", vec![]),
        ];
        let requests = vec![(None, "foo".to_string(), None)];
        let resolved = resolve_install_set(&all, &requests).unwrap();
        assert_eq!(
            resolved,
            vec![(
                "official".to_string(),
                "foo".to_string(),
                "1.1.0".to_string()
            )]
        );
    }

    #[test]
    fn resolves_transitive_dependency() {
        let all = vec![
            pkg("official", "app", "1.0.0", vec![("lib", "^1.0.0")]),
            pkg("official", "lib", "1.0.0", vec![]),
            pkg("official", "lib", "1.5.0", vec![]),
            pkg("official", "lib", "2.0.0", vec![]),
        ];
        let requests = vec![(None, "app".to_string(), None)];
        let mut resolved = resolve_install_set(&all, &requests).unwrap();
        resolved.sort();
        assert_eq!(
            resolved,
            vec![
                (
                    "official".to_string(),
                    "app".to_string(),
                    "1.0.0".to_string()
                ),
                (
                    "official".to_string(),
                    "lib".to_string(),
                    "1.5.0".to_string()
                ),
            ]
        );
    }

    #[test]
    fn constraint_narrows_selected_version() {
        let all = vec![
            pkg("official", "foo", "1.0.0", vec![]),
            pkg("official", "foo", "2.0.0", vec![]),
        ];
        let requests = vec![(None, "foo".to_string(), Some("^1.0.0".to_string()))];
        let resolved = resolve_install_set(&all, &requests).unwrap();
        assert_eq!(
            resolved,
            vec![(
                "official".to_string(),
                "foo".to_string(),
                "1.0.0".to_string()
            )]
        );
    }

    #[test]
    fn conflicting_constraints_produce_dependency_error() {
        let all = vec![
            pkg("official", "app", "1.0.0", vec![("lib", "^2.0.0")]),
            pkg("official", "lib", "1.0.0", vec![]),
        ];
        let requests = vec![(None, "app".to_string(), None)];
        let err = resolve_install_set(&all, &requests).unwrap_err();
        assert!(matches!(
            err,
            ClientError::Core(CoreError::DependencyError(_))
        ));
    }

    #[test]
    fn ambiguous_top_level_request_errors() {
        let all = vec![
            pkg("official", "foo", "1.0.0", vec![]),
            pkg("thirdparty", "foo", "1.0.0", vec![]),
        ];
        let requests = vec![(None, "foo".to_string(), None)];
        let err = resolve_install_set(&all, &requests).unwrap_err();
        assert!(matches!(
            err,
            ClientError::Core(CoreError::AmbiguousPackage(_))
        ));
    }

    #[test]
    fn explicit_source_hint_disambiguates() {
        let all = vec![
            pkg("official", "foo", "1.0.0", vec![]),
            pkg("thirdparty", "foo", "2.0.0", vec![]),
        ];
        let requests = vec![(Some("thirdparty".to_string()), "foo".to_string(), None)];
        let resolved = resolve_install_set(&all, &requests).unwrap();
        assert_eq!(
            resolved,
            vec![(
                "thirdparty".to_string(),
                "foo".to_string(),
                "2.0.0".to_string()
            )]
        );
    }

    #[test]
    fn ambiguous_dependency_name_errors() {
        let all = vec![
            pkg("official", "app", "1.0.0", vec![("lib", "*")]),
            pkg("official", "lib", "1.0.0", vec![]),
            pkg("thirdparty", "lib", "1.0.0", vec![]),
        ];
        let requests = vec![(None, "app".to_string(), None)];
        let err = resolve_install_set(&all, &requests).unwrap_err();
        assert!(matches!(
            err,
            ClientError::Core(CoreError::AmbiguousPackage(_))
        ));
    }

    #[test]
    fn missing_dependency_errors() {
        let all = vec![pkg("official", "app", "1.0.0", vec![("ghost", "*")])];
        let requests = vec![(None, "app".to_string(), None)];
        let err = resolve_install_set(&all, &requests).unwrap_err();
        assert!(matches!(
            err,
            ClientError::Core(CoreError::PackageNotFound(_))
        ));
    }
}
