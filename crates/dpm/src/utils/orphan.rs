use crate::DbPackage;
use std::collections::HashSet;

/// Finds installed packages that were pulled in only as a dependency
/// (`explicit == false`) and are no longer referenced by any other
/// installed package's `dependencies` list. Recurses to a fixpoint: once a
/// package is collected as an orphan, its own dependencies stop counting as
/// "still needed" on the next pass, which can make them orphans too. This
/// terminates because the dependency graph `resolve_install_set`/pubgrub
/// builds is a DAG, so each pass either collects at least one new orphan or
/// the loop ends.
pub fn find_orphans(installed: &[DbPackage]) -> Vec<DbPackage> {
    let mut remaining: Vec<&DbPackage> = installed.iter().collect();
    let mut orphans: Vec<DbPackage> = Vec::new();

    loop {
        let referenced: HashSet<&str> = remaining
            .iter()
            .flat_map(|p| p.dependencies.iter().flatten())
            .map(|dep| dep.name.as_str())
            .collect();

        let (new_orphans, still_needed): (Vec<&DbPackage>, Vec<&DbPackage>) = remaining
            .into_iter()
            .partition(|p| !p.explicit && !referenced.contains(p.name.as_str()));

        if new_orphans.is_empty() {
            break;
        }
        orphans.extend(new_orphans.into_iter().cloned());
        remaining = still_needed;
    }

    orphans
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pkg(name: &str, explicit: bool, deps: &[&str]) -> DbPackage {
        let dependencies = if deps.is_empty() {
            None
        } else {
            Some(
                deps.iter()
                    .map(|d| dpm_core::Dependency {
                        name: d.to_string(),
                        version: "*".to_string(),
                    })
                    .collect(),
            )
        };
        DbPackage::new(
            "official",
            name,
            "1.0.0",
            "prebuilt",
            None,
            None,
            None,
            None,
            "",
            None,
            dependencies,
            None,
            None,
            explicit,
        )
    }

    #[test]
    fn explicit_package_is_never_an_orphan_even_if_unreferenced() {
        let installed = vec![pkg("a", true, &[])];
        assert!(find_orphans(&installed).is_empty());
    }

    #[test]
    fn auto_package_still_referenced_is_not_an_orphan() {
        let installed = vec![pkg("a", true, &["b"]), pkg("b", false, &[])];
        assert!(find_orphans(&installed).is_empty());
    }

    #[test]
    fn single_level_orphan() {
        // The package that used to depend on "b" has already been
        // uninstalled (removed from the installed set) — only "b" remains,
        // now unreferenced.
        let installed = vec![pkg("b", false, &[])];
        let orphans = find_orphans(&installed);
        assert_eq!(orphans.len(), 1);
        assert_eq!(orphans[0].name, "b");
    }

    #[test]
    fn transitive_multi_level_orphan() {
        // "b" depends on "c", both auto-installed, nothing left depends on "b".
        let installed = vec![pkg("b", false, &["c"]), pkg("c", false, &[])];
        let mut names: Vec<String> = find_orphans(&installed)
            .into_iter()
            .map(|p| p.name)
            .collect();
        names.sort();
        assert_eq!(names, vec!["b".to_string(), "c".to_string()]);
    }

    #[test]
    fn empty_installed_set_has_no_orphans() {
        assert!(find_orphans(&[]).is_empty());
    }
}
