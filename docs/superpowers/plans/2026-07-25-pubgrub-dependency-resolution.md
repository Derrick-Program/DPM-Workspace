# Pubgrub Dependency Resolution (Phase 5) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace `dpm install`'s current "resolve each requested package independently, always pick the newest version" logic with real dependency-graph resolution: `[source/]name[@constraint]` CLI syntax, joint solving of all requested packages plus their transitive `dependencies` via the `pubgrub` crate, and clear conflict reporting when no solution exists.

**Architecture:** All resolution logic lives in the `dpm` client crate only (`dpm-core` stays data-only, `dpm-server` is untouched — this phase's spec section explicitly scopes it to `dpm-core`/`dpm`). A new `crates/dpm/src/utils/resolver.rs` owns: (a) parsing a version string into `pubgrub::SemanticVersion` and a constraint string into `pubgrub::Ranges<SemanticVersion>` (built on top of `semver::VersionReq`'s parser, since hand-rolling caret/tilde parsing would duplicate a well-tested library), (b) a `PkgId(source, name)` newtype used as pubgrub's package identifier, and (c) `resolve_install_set()`, which loads the *entire* local index (`get_db().read_all()` — already populated by `dpm update`) into an in-memory `pubgrub::OfflineDependencyProvider` and calls `pubgrub::resolve()` once for the whole requested set (a synthetic root package depends on every CLI-requested package). `action.rs::install()` is rewired to call this once instead of looping `sources_of`/`latest_version` per package.

**Tech Stack:** `pubgrub = "0.4"` (crates.io, released 2026-04-09), `semver = "1"` (crates.io) — both added to `crates/dpm/Cargo.toml` only.

## Global Constraints

These are exact facts about the `pubgrub` 0.4.0 API, verified against the actual crate source at `~/.cargo/registry/src/*/pubgrub-0.4.0/src/` (NOT assumed from memory — 0.4.0 changed some return types from the 0.3.0 docs you may find online). Every task below depends on these being correct as stated:

- `pubgrub::SemanticVersion` is `{ major: u32, minor: u32, patch: u32 }` — no pre-release/build metadata field. `semver::Version` (the `semver` crate) has `major/minor/patch: u64` plus `pre`/`build`. Converting requires rejecting non-empty `pre`/`build` (dpm has no pre-release version convention anywhere today) and narrowing `u64 -> u32` with a bounds check.
- `pubgrub::Package` is auto-implemented for any type that is `Clone + Eq + Hash + Debug + Display`. **A bare `(String, String)` tuple does NOT implement `Display`** — you must use a newtype (`PkgId`) with a manual `Display` impl.
- `pubgrub::OfflineDependencyProvider<P, VS>::add_dependencies<I: IntoIterator<Item = (P, VS)>>(&mut self, package: P, version: impl Into<V>, dependencies: I)` — one call per `(package, version)` pair; a second call for the same pair *replaces* its dependencies (don't call it twice for the same key).
- `OfflineDependencyProvider::choose_version` picks the **highest** version satisfying the requested range (verified in `provider.rs`: `versions.keys().rev().find(|v| range.contains(v))` over an internal `BTreeMap<V, _>`) — this preserves today's "install always gets the newest compatible version" behavior with zero custom code.
- `pubgrub::resolve<DP: DependencyProvider>(dependency_provider: &DP, package: DP::P, version: impl Into<DP::V>) -> Result<SelectedDependencies<DP::P, DP::V>, PubGrubError<DP>>`.
- `SelectedDependencies<P, V>` (opaque as of 0.4.0) supports `.iter() -> impl Iterator<Item = (&P, &V)>`, `.get(&P) -> Option<&V>`, and `IntoIterator` yielding owned `(P, V)` — use `.into_iter()` to get owned pairs.
- `pubgrub::Ranges<V>` (re-exported from the `version-ranges` crate) constructors used in this plan: `Ranges::full()`, `Ranges::empty()`, `Ranges::singleton(v)`, `Ranges::higher_than(v)`, `Ranges::strictly_higher_than(v)`, `Ranges::lower_than(v)`, `Ranges::strictly_lower_than(v)`, `Ranges::between(a, b)` (`[a, b)`), and `.intersection(&other)`.
- `PubGrubError<DP>` variants: `NoSolution(NoSolutionError<DP>)`, `ErrorRetrievingDependencies { package, version, source }`, `ErrorChoosingVersion { package, source }`, `ErrorInShouldCancel(source)`. All four derive `thiserror::Error` (have a `Display` impl via `#[error(...)]`). `NoSolutionError` has `.collapse_no_versions(&mut self)`; `pubgrub::DefaultStringReporter::report(&tree) -> String` (trait `pubgrub::Reporter`) produces the human-readable explanation.
- Package identity is the `(source, name)` pair (matches the design spec's Section 5 point 4: "package identifier 用 (source, name) tuple, 維持跟 install 解析同一套 collision-safety 規則"). A bare dependency name inside a package's `dependencies: Vec<Dependency>` list has **no `source` field** — resolving *which* source a dependency name refers to uses the exact same 0/1/2+-sources rule as top-level CLI install targets (0 sources → `CoreError::PackageNotFound`, 2+ → `CoreError::AmbiguousPackage`, exactly 1 → use it). This is a real, permanent limitation: a dependency name that is ambiguous across sources cannot currently be disambiguated by the depending package's author (no `Dependency.source` field exists) — this plan does not add one (schema change, out of scope, note it in TODO.md in Task 4).
- Scope decision (spec Section 5 point 6 explicitly leaves this to implementation time): this phase only resolves the CLI-requested install set plus its transitive dependencies **fresh** — it does **not** look at what's already installed on disk as an additional constraint (today's code doesn't do this either). `upgrade`/`uninstall`/`search` subcommands are **not** touched by this phase — they keep treating their `PN` argument as a bare package name, no `[source/]name[@constraint]` parsing. Only `install` gets the new syntax.
- Reuse `CoreError::DependencyError(String)` (already exists in `crates/dpm-core/src/error.rs`, currently unused) for every resolution-failure case (constraint parse errors, pubgrub `NoSolution` reports, ambiguous/missing dependency names) — do not add a new `CoreError`/`ClientError` variant.
- `DbPackage` (in `crates/dpm/src/utils/models.rs`) has fields `source, name, version, kind, url, hash, filename, build_command, description, entry, dependencies: Option<Vec<Dependency>>` (all `String` except `dependencies`). `Db::read_all(&self) -> ClientResult<Vec<DbPackage>>` already exists and returns every row across every source.
- `Dependency` (in `crates/dpm-core/src/lib.rs`) is `{ name: String, version: String }` — `version` here is a **constraint string** (e.g. `"^1.2.0"`), not an exact pin.

---

### Task 1: Version and constraint parsing (`crates/dpm/src/utils/resolver.rs`)

**Files:**
- Modify: `crates/dpm/Cargo.toml` (add `pubgrub`, `semver` dependencies)
- Create: `crates/dpm/src/utils/resolver.rs`
- Modify: `crates/dpm/src/utils/mod.rs` (register the new module)

**Interfaces:**
- Produces: `pub fn parse_version(s: &str) -> ClientResult<pubgrub::SemanticVersion>`, `pub fn parse_constraint(s: &str) -> ClientResult<pubgrub::Ranges<pubgrub::SemanticVersion>>` — Task 2 and Task 3 both call these.

- [ ] **Step 1: Add dependencies**

Edit `crates/dpm/Cargo.toml`, in the `[dependencies]` section add:

```toml
pubgrub = "0.4"
semver = "1"
```

Run `cargo check -p DPM` (the client crate's package name is `DPM`, confirm with `grep '^name' crates/dpm/Cargo.toml` if unsure) to confirm they resolve and build.

- [ ] **Step 2: Write the failing tests for `parse_version`**

Create `crates/dpm/src/utils/resolver.rs` with:

```rust
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
}
```

- [ ] **Step 3: Run the tests to verify they pass**

Run: `cargo test -p DPM resolver::tests -- --nocapture`
Expected: 3 passed (compiles because `pubgrub`/`semver` are now real dependencies; `parse_version` is fully implemented already — this step is verifying, not red/green TDD, since the function is small enough to write correctly in one pass. If any test fails, fix `parse_version` before continuing.)

- [ ] **Step 4: Implement `parse_constraint`**

Append to `crates/dpm/src/utils/resolver.rs` (after `parse_version`, before the `#[cfg(test)]` module):

```rust
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
```

- [ ] **Step 5: Write tests for `parse_constraint`**

Append to the `#[cfg(test)] mod tests` block from Step 2 (same file):

```rust
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
```

- [ ] **Step 6: Run tests, verify all pass**

Run: `cargo test -p DPM resolver::tests`
Expected: all tests (Step 2's 3 + Step 5's 8) pass.

- [ ] **Step 7: Register the module**

Read `crates/dpm/src/utils/mod.rs` first to match its existing style (it should look like `pub mod db; pub use self::db::*;` repeated per file — confirm before editing). Add:

```rust
pub mod resolver;
pub use self::resolver::*;
```

Run `cargo check -p DPM` to confirm the crate still compiles as a whole (this surfaces unused-import warnings from the new `pub use`, which is expected until Task 2/3 consume `parse_version`/`parse_constraint` elsewhere — do not silence them with `#[allow(dead_code)]`, they'll go away naturally once Task 2 lands).

- [ ] **Step 8: Commit**

```bash
git add crates/dpm/Cargo.toml crates/dpm/src/utils/resolver.rs crates/dpm/src/utils/mod.rs
git commit -m "feat(dpm): parse version/constraint strings into pubgrub SemanticVersion/Ranges"
```

---

### Task 2: `PkgId`, CLI spec parsing, and `resolve_install_set()`

**Files:**
- Modify: `crates/dpm/src/utils/resolver.rs` (append to the file Task 1 created)

**Interfaces:**
- Consumes: `parse_version(&str) -> ClientResult<SemanticVersion>`, `parse_constraint(&str) -> ClientResult<Ranges<SemanticVersion>>` (Task 1). `DbPackage { source, name, version, dependencies: Option<Vec<Dependency>>, .. }` (`crates/dpm/src/utils/models.rs`, pre-existing). `Dependency { name, version }` (`dpm_core`, pre-existing).
- Produces: `pub fn parse_package_spec(spec: &str) -> (Option<&str>, &str, Option<&str>)`, `pub fn resolve_install_set(all_packages: &[DbPackage], requests: &[(Option<String>, String, Option<String>)]) -> ClientResult<Vec<(String, String, String)>>` (returns `(source, name, version_string)` triples, root excluded) — Task 3's `action.rs::install()` calls both.

- [ ] **Step 1: Write `parse_package_spec` and its tests**

Append to `crates/dpm/src/utils/resolver.rs` (above the `#[cfg(test)]` module):

```rust
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
```

Test (append to the `mod tests` block):

```rust
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
```

Run: `cargo test -p DPM resolver::tests::spec_` — expect 4 passed.

- [ ] **Step 2: `PkgId` newtype**

Append to `crates/dpm/src/utils/resolver.rs` (above `#[cfg(test)]`):

```rust
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
```

- [ ] **Step 3: Dependency-name-to-source resolution helper**

Append to `crates/dpm/src/utils/resolver.rs` (above `#[cfg(test)]`). This implements the same 0/1/2+ rule the spec requires for both top-level CLI targets and bare dependency names. This is also the first code in this file that references `DbPackage`, so add the import at the top of the file (next to the existing `use crate::{ClientError, ClientResult};` line from Task 1 — change it to `use crate::{ClientError, ClientResult, DbPackage};`):

```rust
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
```

- [ ] **Step 4: `resolve_install_set`**

Append to `crates/dpm/src/utils/resolver.rs` (above `#[cfg(test)]`):

```rust
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
```

- [ ] **Step 5: Write tests for `resolve_install_set`**

Append to the `#[cfg(test)] mod tests` block. These build `DbPackage` fixtures directly (no real database needed — `resolve_install_set` is a pure function over `&[DbPackage]`):

```rust
    use dpm_core::Dependency;

    fn pkg(
        source: &str,
        name: &str,
        version: &str,
        deps: Vec<(&str, &str)>,
    ) -> DbPackage {
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
            vec![("official".to_string(), "foo".to_string(), "1.1.0".to_string())]
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
                ("official".to_string(), "app".to_string(), "1.0.0".to_string()),
                ("official".to_string(), "lib".to_string(), "1.5.0".to_string()),
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
            vec![("official".to_string(), "foo".to_string(), "1.0.0".to_string())]
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
            vec![("thirdparty".to_string(), "foo".to_string(), "2.0.0".to_string())]
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
```

- [ ] **Step 6: Run tests, verify all pass**

Run: `cargo test -p DPM resolver::`
Expected: every test from Task 1 and Task 2 passes (Task 1's 11 + Task 2's 4 spec tests + 8 resolve tests = 23 total; exact count isn't the point, zero failures is).

- [ ] **Step 7: Commit**

```bash
git add crates/dpm/src/utils/resolver.rs
git commit -m "feat(dpm): PkgId + resolve_install_set() over pubgrub OfflineDependencyProvider"
```

---

### Task 3: Wire `resolve_install_set` into `action.rs::install()`

**Files:**
- Modify: `crates/dpm/src/action.rs`

**Interfaces:**
- Consumes: `parse_package_spec(&str) -> (Option<&str>, &str, Option<&str>)`, `resolve_install_set(&[DbPackage], &[(Option<String>, String, Option<String>)]) -> ClientResult<Vec<(String, String, String)>>` (Task 2, both re-exported from `crate::utils` via `pub use self::resolver::*;`).
- Produces: `install()`'s public signature and behavior for callers is unchanged (still `pub async fn install(&self) -> ClientResult<()>`) — only its internals and the accepted CLI argument syntax change.

Read `crates/dpm/src/action.rs` in full before starting — `install()` and `parse_mine()` are the only two functions this task touches, but `install_source_package` (called unchanged from inside the new `install()`) and the prebuilt-install code both stay as they are; you're changing how `repo_package_info` gets picked, not what happens once it's picked.

- [ ] **Step 1: Rewrite `parse_mine` to operate on parsed names and take a pre-fetched snapshot**

Replace the existing `parse_mine` method:

```rust
    async fn parse_mine(&self) -> (Vec<String>, Vec<String>) {
        let mut is: Vec<String> = Vec::new();
        let mut isnot: Vec<String> = Vec::new();
        let all_packages = get_db().read_all().await.unwrap_or_else(|_| Vec::new());
        let package_names: Vec<String> = all_packages.into_iter().map(|pkg| pkg.name).collect();
        for pkg in &self.pkgs {
            if package_names.contains(pkg) {
                is.push(pkg.clone());
            } else {
                isnot.push(pkg.clone());
            }
        }
        (is, isnot)
    }
```

with:

```rust
    /// Splits `self.pkgs` (raw `[source/]name[@constraint]` strings) into
    /// packages known to at least one configured source (`is`, parsed into
    /// `(source_hint, name, constraint)` triples for `resolve_install_set`)
    /// and packages not found locally at all (`isnot`, falls through to the
    /// OS package manager) — same split as before, just keyed off the
    /// parsed bare `name` instead of the raw spec string, since a spec like
    /// `official/foo@^1.0` will never literally equal a DB `name` column.
    fn parse_mine(
        &self,
        all_packages: &[DbPackage],
    ) -> (Vec<(Option<String>, String, Option<String>)>, Vec<String>) {
        let mut is = Vec::new();
        let mut isnot = Vec::new();
        for raw in &self.pkgs {
            let (source, name, constraint) = parse_package_spec(raw);
            if all_packages.iter().any(|p| p.name == name) {
                is.push((
                    source.map(str::to_string),
                    name.to_string(),
                    constraint.map(str::to_string),
                ));
            } else {
                isnot.push(name.to_string());
            }
        }
        (is, isnot)
    }
```

- [ ] **Step 2: Rewrite the resolution portion of `install()`**

Replace:

```rust
    pub async fn install(&self) -> ClientResult<()> {
        let (is, isnot) = self.parse_mine().await;
        if !is.is_empty() {
            for pkg in is {
                let pkg = pkg.as_str();
                let sources = get_db()
                    .sources_of(pkg)
                    .await
                    .map_err(|e| ClientError::Core(CoreError::DatabaseError(e.to_string())))?;
                let source_alias = match sources.len() {
                    0 => {
                        return Err(ClientError::Core(CoreError::PackageNotFound(
                            pkg.to_string(),
                        )))
                    }
                    1 => sources.into_iter().next().unwrap(),
                    _ => {
                        return Err(ClientError::Core(CoreError::AmbiguousPackage(format!(
                            "{pkg} (found in: {})",
                            sources.join(", ")
                        ))))
                    }
                };
                let repo_package_info = get_db()
                    .latest_version(&source_alias, pkg)
                    .await
                    .map_err(|e| ClientError::Core(CoreError::DatabaseError(e.to_string())))?
                    .ok_or_else(|| {
                        ClientError::Core(CoreError::PackageNotFound(pkg.to_string()))
                    })?;
                if self.verbose {
                    println!("{}\n\n  {}", pkg.on_green(), "Downloading...".yellow());
                }
```

with:

```rust
    pub async fn install(&self) -> ClientResult<()> {
        let all_packages = get_db()
            .read_all()
            .await
            .map_err(|e| ClientError::Core(CoreError::DatabaseError(e.to_string())))?;
        let (is, isnot) = self.parse_mine(&all_packages);
        if !is.is_empty() {
            let resolved = resolve_install_set(&all_packages, &is)?;
            for (source_alias, name, version) in resolved {
                let pkg = name.as_str();
                let repo_package_info = all_packages
                    .iter()
                    .find(|p| p.source == source_alias && p.name == name && p.version == version)
                    .ok_or_else(|| {
                        ClientError::Core(CoreError::PackageNotFound(format!(
                            "{source_alias}/{name}@{version}"
                        )))
                    })?;
                if self.verbose {
                    println!("{}\n\n  {}", pkg.on_green(), "Downloading...".yellow());
                }
```

Everything from the original `let filename = repo_package_info.filename...` line through the end of the `for` loop body (the prebuilt-install path, and the `if repo_package_info.kind == "source" { self.install_source_package(...); continue; }` branch above it) stays **exactly as it is today** — `repo_package_info` is still a `&DbPackage` and `pkg`/`source_alias` are still `&str`-compatible (`pkg` is `&str` via `name.as_str()`; `source_alias` is now a plain `String` — everywhere the old code passed `&source_alias`, that still works since `String` derefs to `&str`).

Do not change the `if !isnot.is_empty() { for pkg in isnot { self.system_action.install_package(&pkg)?; } }` block — `isnot` is still `Vec<String>` of bare names, unchanged.

- [ ] **Step 3: Build and run the existing test suite**

Run: `cargo build -p DPM && cargo test -p DPM`
Expected: builds clean, all existing tests (13 dpm lib tests per the last full-suite run, plus Task 1/2's new `resolver::` tests) still pass. Pay attention to any borrow-checker complaint about `all_packages` being borrowed by `repo_package_info` while also being read earlier in `parse_mine` — `parse_mine` takes `&all_packages` and returns owned data, so this should not conflict, but confirm by actually compiling rather than assuming.

- [ ] **Step 4: Manual smoke test (no real network needed)**

This exercises the new CLI syntax end to end against the local index only (no install of a real package required — a `PackageNotFound`/`AmbiguousPackage` error still proves the parsing+resolution wiring works):

```bash
just run-client -- update
just run-client -- install nonexistent-package-xyz
# Expected: falls through to the OS package manager attempt (isnot path) or
# a clear "Package not found" error - NOT a panic or a parse error about '@'/'/'.
just run-client -- install official/nonexistent-package-xyz@^1.0
# Expected: PackageNotFound or AmbiguousPackage-style error naming
# "official/nonexistent-package-xyz", not a crash.
```

- [ ] **Step 5: Commit**

```bash
git add crates/dpm/src/action.rs
git commit -m "feat(dpm): install() resolves the whole requested set jointly via pubgrub"
```

---

### Task 4: Documentation

**Files:**
- Modify: `README.md`
- Modify: `TODO.md` (only if it currently lists pubgrub/dependency-resolution as outstanding — check first)

**Interfaces:**
- None (doc-only task).

- [ ] **Step 1: Update README's client subcommand section**

In `README.md`, under `### 子指令` (the client subcommand table), update the `install` row's example and add a new subsection right after the table (before `### 套件種類:Prebuilt vs Source`) explaining the new syntax:

```markdown
### 相依解析(pubgrub)

`install` 支援 `[source/]name[@constraint]` 語法(比照 npm):

- `dpm install foo` —— 名字沒指定來源,跟之前一樣走 0/1/2+ 來源數規則自動解析或報錯
- `dpm install official/foo` —— 明確指定來源
- `dpm install foo@^1.2` —— 版本約束,`^`/`~`/比較運算子沿用 Cargo 風格語意(`^1.2.3`、`~1.2.3`、`>=1.0.0, <2.0.0`);裸版號(`1.2.3`,沒有任何前綴)是 **npm 風格的精確釘版本**,不是 Cargo.toml 那種「裸版號等於 `^`」——因為 `1.2.3`/`^1.2.3` 在 `semver::VersionReq` 解析後都是同一個 `Op::Caret`,無法事後分辨,dpm 在丟進 `VersionReq` 前就把純數字+點的裸字串改寫成 `=1.2.3` 明確精確比對。不寫約束預設 `*`(任何版本)。
- `dpm install official/foo@^1.2` —— 來源 + 約束一起寫

一次 `install` 裝多個套件時,所有套件(以及它們各自的 `dependencies`)會一起丟給 `pubgrub` 做**聯合求解**——不是每個套件獨立挑「目前最新版」,而是在滿足全部套件、全部相依限制的前提下,每個套件仍然挑得到的最新版本。任兩個套件的相依限制衝突(例如 A 需要 `lib@^2.0`、B 需要 `lib@^1.0`)會直接報錯並印出 `pubgrub` 產生的衝突鏈說明,不會裝一半。

已知限制:套件的 `dependencies` 欄位只有 `name`+版本約束,没有 `source` 欄位——如果某個相依名稱同時存在於多個來源,現在無法在該相依關係裡指定要哪個來源,會直接報 `AmbiguousPackage`(跟 CLI 上裝到同名衝突套件的報錯規則一致)。`upgrade`/`uninstall`/`search` 目前還是吃純套件名,沒有 `source/name@constraint` 語法。
```

- [ ] **Step 2: Check and update TODO.md**

Run `grep -n "pubgrub" TODO.md`. If it lists pubgrub/dependency-resolution as an open item, mark it resolved (matching how Phase 4's `git2` TODO item was closed in commit `764c56d`). If no such entry exists, skip this step — don't invent a TODO entry just to close it.

- [ ] **Step 3: Commit**

```bash
git add README.md TODO.md
git commit -m "docs: document pubgrub-backed install() dependency resolution"
```

(If Step 2 found no TODO.md change to make, omit it from `git add`.)
