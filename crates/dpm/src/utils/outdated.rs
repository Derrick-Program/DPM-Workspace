use crate::{parse_version, DbPackage};

/// One installed package that has a newer version available from the same
/// source it was installed from.
pub struct OutdatedPackage {
    pub name: String,
    pub source: String,
    pub current: String,
    pub latest: String,
}

/// For each installed package, finds the highest semver version of the same
/// `(source, name)` pair in `available` and reports it if it's newer than
/// what's installed. Only compares within the installed package's own
/// source — an installed package's source is fixed at install time (see
/// `DbPackage::source`), so there's no ambiguity to resolve the way
/// `resolve_install_set` has to for a fresh install.
///
/// Version strings that fail to parse (either side) are skipped rather than
/// failing the whole scan — a single malformed entry in the index shouldn't
/// hide every other outdated package.
pub fn find_outdated(installed: &[DbPackage], available: &[DbPackage]) -> Vec<OutdatedPackage> {
    let mut outdated = Vec::new();
    for pkg in installed {
        let Ok(current) = parse_version(&pkg.version) else {
            continue;
        };
        let latest = available
            .iter()
            .filter(|p| p.source == pkg.source && p.name == pkg.name)
            .filter_map(|p| parse_version(&p.version).ok().map(|v| (v, &p.version)))
            .max_by_key(|(v, _)| *v);
        if let Some((latest_version, latest_str)) = latest {
            if latest_version > current {
                outdated.push(OutdatedPackage {
                    name: pkg.name.clone(),
                    source: pkg.source.clone(),
                    current: pkg.version.clone(),
                    latest: latest_str.clone(),
                });
            }
        }
    }
    outdated
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pkg(source: &str, name: &str, version: &str) -> DbPackage {
        DbPackage::new(
            source, name, version, "prebuilt", None, None, None, None, "", None, None, None, None,
            true,
        )
    }

    #[test]
    fn reports_package_with_newer_version_in_same_source() {
        let installed = vec![pkg("official", "foo", "1.2.0")];
        let available = vec![
            pkg("official", "foo", "1.2.0"),
            pkg("official", "foo", "1.3.0"),
        ];

        let result = find_outdated(&installed, &available);

        assert_eq!(result.len(), 1);
        assert_eq!(result[0].name, "foo");
        assert_eq!(result[0].current, "1.2.0");
        assert_eq!(result[0].latest, "1.3.0");
    }

    #[test]
    fn up_to_date_package_is_not_reported() {
        let installed = vec![pkg("official", "foo", "1.3.0")];
        let available = vec![
            pkg("official", "foo", "1.2.0"),
            pkg("official", "foo", "1.3.0"),
        ];

        assert!(find_outdated(&installed, &available).is_empty());
    }

    #[test]
    fn ignores_newer_version_from_a_different_source() {
        let installed = vec![pkg("official", "foo", "1.2.0")];
        let available = vec![pkg("third-party", "foo", "9.0.0")];

        assert!(
            find_outdated(&installed, &available).is_empty(),
            "a newer version in another source must not count — the \
             installed package's source is fixed"
        );
    }

    #[test]
    fn skips_installed_package_with_unparsable_version() {
        let installed = vec![pkg("official", "foo", "not-a-version")];
        let available = vec![pkg("official", "foo", "1.3.0")];

        assert!(find_outdated(&installed, &available).is_empty());
    }

    #[test]
    fn package_missing_from_available_index_is_not_reported() {
        let installed = vec![pkg("official", "gone", "1.0.0")];
        let available = vec![];

        assert!(find_outdated(&installed, &available).is_empty());
    }
}
