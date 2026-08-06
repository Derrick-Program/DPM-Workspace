# dpm autoremove Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Give `dpm` an `explicit`/auto install-reason flag on installed packages, a `find_orphans` function that uses it, and a `dpm autoremove` command that removes orphaned dependencies (plus a hint printed after `dpm uninstall`).

**Architecture:** `InstalledPackages` gains an `explicit INTEGER NOT NULL DEFAULT 1` column. `dpm install`/`upgrade` write it based on whether the package was named directly on the command line (vs. pulled in by `resolve_install_set`), using an upsert so re-installing an already-auto-installed package can promote it to explicit but never demotes it back. A new pure function `find_orphans` walks the installed set's `dependencies` to find `explicit == false` packages nothing depends on anymore, recursing to a fixpoint. `dpm uninstall` prints a hint when it creates new orphans; `dpm autoremove` lists and removes them. Both removal paths share one `ActionInfo::remove_installed_package` helper, replacing `uninstall()`'s current unparameterized `DELETE ... WHERE name = '{pkg}'` string-formatting with a parameterized `Db::delete_installed`.

**Tech Stack:** Rust, `turso` (SQLite-compatible, async), `clap` derive CLI, `tokio::test`/`#[test]` for tests.

## Global Constraints

- Spec: `docs/superpowers/specs/2026-08-06-autoremove-design.md`. Every task below implements a section of it — do not invent behavior not in the spec.
- No interactive confirmation prompts anywhere (`dpm autoremove` lists then removes directly, matching the existing `install`/`uninstall` no-prompt convention).
- `explicit` promotes `false → true` on a direct re-install, never demotes `true → false` automatically (no "unmark as explicit" command exists yet — out of scope).
- Every task ends with `cargo check --workspace` and the relevant `cargo test` passing before commit. Run `cargo fmt --all` before each commit if the diff touches formatting-sensitive code.
- Follow existing code style exactly: `ClientError::Core(CoreError::DatabaseError(...))` / `ClientError::Core(DatabaseError(...))` error wrapping, `colored::Colorize` for CLI output, doc comments in Traditional Chinese where the surrounding file already uses Chinese comments (`db.rs`, `action.rs`), English where the surrounding file uses English.

---

### Task 1: `explicit` column on `InstalledPackages` + DB layer

**Files:**
- Modify: `crates/dpm/src/utils/models.rs` (`DbPackage` struct + `DbPackage::new`)
- Modify: `crates/dpm/src/utils/db.rs` (`run_migrations`, `insert`, `read_all`, `read_one`, `row_to_package` split, new `delete_installed`)
- Modify (mechanical, add trailing `true` arg to `DbPackage::new` calls): `crates/dpm/src/action.rs:501,1673,1730,1774,1804,1819`, `crates/dpm/src/context.rs:263`, `crates/dpm/src/utils/resolver.rs:477`, `crates/dpm/src/utils/fetcher.rs:167`, `crates/dpm/tests/db_tests.rs:22-38` (`sample_pkg` helper)
- Test: `crates/dpm/tests/db_tests.rs`

**Interfaces:**
- Produces: `DbPackage.explicit: bool` field. `DbPackage::new(..., explicit: bool)` — 14th positional param, added at the end. `Db::delete_installed(&self, name: &str) -> ClientResult<()>`. `Db::insert` now upserts on `name` conflict and never demotes `explicit` from `true`.

- [ ] **Step 1: Add `explicit` field to `DbPackage` and thread it through the constructor + every call site**

In `crates/dpm/src/utils/models.rs`, add the field and constructor param:

```rust
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct DbPackage {
    pub source: String,
    pub name: String,
    pub version: String,
    /// "prebuilt" | "source"
    pub kind: String,
    pub url: Option<String>,
    pub hash: Option<String>,
    pub filename: Option<String>,
    pub build_command: Option<String>,
    pub description: String,
    pub entry: Option<String>,
    pub dependencies: Option<Vec<Dependency>>,
    /// 發布這個版本的作者 id——只有官方來源(`repo_url == OFFICIAL_REPO_URL`)
    /// 的套件才會有值,第三方來源永遠是 `None`。
    pub author: Option<String>,
    /// `dpm-server sign` 簽出來的 hex 簽章,簽的是 `hash` 欄位。
    pub signature: Option<String>,
    /// 使用者是否在命令列直接指名安裝這個套件(`true`),還是被別的套件的
    /// `dependencies` 拉進來裝的(`false`)。只對 `InstalledPackages` 有意義
    /// ——`AvailablePackages`(遠端索引快取)裡的列這個欄位恆為 `true`,沒有
    /// 實際語意,純粹因為 `DbPackage` 是兩張表共用的資料結構。`dpm autoremove`
    /// 靠這個欄位判斷哪些已裝套件可以被當成孤兒依賴清掉。
    pub explicit: bool,
}
```

```rust
#[allow(clippy::too_many_arguments)]
impl DbPackage {
    pub fn new(
        source: &str,
        name: &str,
        version: &str,
        kind: &str,
        url: Option<String>,
        hash: Option<String>,
        filename: Option<String>,
        build_command: Option<String>,
        description: &str,
        entry: Option<String>,
        dependencies: Option<Vec<Dependency>>,
        author: Option<String>,
        signature: Option<String>,
        explicit: bool,
    ) -> Self {
        DbPackage {
            source: source.to_owned(),
            name: name.to_owned(),
            version: version.to_owned(),
            kind: kind.to_owned(),
            url,
            hash,
            filename,
            build_command,
            description: description.to_owned(),
            entry,
            dependencies,
            author,
            signature,
            explicit,
        }
    }
```

Now fix every call site by appending `true` (or, for `db_tests.rs`'s `sample_pkg`, see below) as the 14th argument:

`crates/dpm/src/action.rs:501` — inside `sync_source_inner`'s `insert_available(DbPackage::new(` call, add `true,` after the `signature,` line (before the closing `))`).

`crates/dpm/src/action.rs:1673`, `1730`, `1774`, `1804`, `1819` — each of these five `DbPackage::new(` test fixtures ends its argument list with a `None,`/`Some(...)`, line for `signature` followed by `);`. Add `true,` as a new line right before each closing `);`.

`crates/dpm/src/context.rs:263` — the compact single-line-per-arg call:
```rust
.insert(crate::DbPackage::new(
    "official", "foo", "1.0.0", "prebuilt", None, None, None, None, "", None, None,
    None, None,
))
```
becomes:
```rust
.insert(crate::DbPackage::new(
    "official", "foo", "1.0.0", "prebuilt", None, None, None, None, "", None, None,
    None, None, true,
))
```

`crates/dpm/src/utils/resolver.rs:477` — the `DbPackage::new(` fixture used by `resolver.rs`'s own tests: add `true,` as a new final line before the closing `)`.

`crates/dpm/src/utils/fetcher.rs:167` — same pattern, add `true,` before the closing `)`.

`crates/dpm/tests/db_tests.rs:22-38` — the `sample_pkg` helper:
```rust
fn sample_pkg(source: &str, version: &str) -> DbPackage {
    DbPackage::new(
        source,
        "test_pkg",
        version,
        "prebuilt",
        Some("http://example.com".to_string()),
        Some("1234567890abcdef".to_string()),
        Some("test_pkg.tar.gz".to_string()),
        None,
        "A test package",
        Some("bin/test_pkg".to_string()),
        None,
        None,
        None,
        true,
    )
}
```

Run `cargo check --workspace` and fix any remaining call sites the grep above missed (search again with `grep -rn "DbPackage::new" crates/` if `cargo check` reports more).

- [ ] **Step 2: Write failing tests for schema persistence and upsert semantics**

Add to `crates/dpm/tests/db_tests.rs`, inside the `mod db_tests` block:

```rust
    #[tokio::test]
    async fn test_explicit_flag_persists_and_defaults_true() -> TestResult {
        let dir = tempdir()?;
        let db = setup_db(dir.path(), false).await?;
        let mut pkg = sample_pkg("official", "0.1.0");
        pkg.explicit = false;
        db.insert(pkg).await?;

        let all = db.read_all().await?;
        assert_eq!(all.len(), 1);
        assert!(!all[0].explicit);
        Ok(())
    }

    #[tokio::test]
    async fn test_reinstall_same_name_upserts_and_sticky_promotes_explicit() -> TestResult {
        let dir = tempdir()?;
        let db = setup_db(dir.path(), false).await?;

        let mut auto_pkg = sample_pkg("official", "0.1.0");
        auto_pkg.explicit = false;
        db.insert(auto_pkg).await?;

        let mut upgraded = sample_pkg("official", "0.2.0");
        upgraded.explicit = false;
        db.insert(upgraded).await?;
        let after_auto_reinstall = db.read_all().await?;
        assert_eq!(
            after_auto_reinstall.len(),
            1,
            "same name must upsert, not duplicate"
        );
        assert_eq!(after_auto_reinstall[0].version, "0.2.0");
        assert!(!after_auto_reinstall[0].explicit);

        let mut explicit_reinstall = sample_pkg("official", "0.2.0");
        explicit_reinstall.explicit = true;
        db.insert(explicit_reinstall).await?;
        let after_explicit_reinstall = db.read_all().await?;
        assert!(
            after_explicit_reinstall[0].explicit,
            "a direct re-install must promote explicit"
        );

        let mut dep_only_reinstall = sample_pkg("official", "0.3.0");
        dep_only_reinstall.explicit = false;
        db.insert(dep_only_reinstall).await?;
        let after_dep_only_reinstall = db.read_all().await?;
        assert!(
            after_dep_only_reinstall[0].explicit,
            "explicit must never auto-demote back to false"
        );
        Ok(())
    }

    #[tokio::test]
    async fn test_run_migrations_is_idempotent_for_the_explicit_column() -> TestResult {
        let dir = tempdir()?;
        let db = setup_db(dir.path(), false).await?;
        // A second run must not error on `ALTER TABLE ... ADD COLUMN explicit`
        // finding the column already there — this is what protects DBs that
        // already had `InstalledPackages` before this column existed.
        db.run_migrations(false).await?;
        db.insert(sample_pkg("official", "0.1.0")).await?;
        assert!(db.read_all().await?[0].explicit);
        Ok(())
    }

    #[tokio::test]
    async fn test_delete_installed_removes_only_the_named_package() -> TestResult {
        let dir = tempdir()?;
        let db = setup_db(dir.path(), false).await?;
        db.insert(sample_pkg("official", "0.1.0")).await?;
        let mut other = sample_pkg("official", "0.1.0");
        other.name = "other_pkg".to_string();
        db.insert(other).await?;

        db.delete_installed("test_pkg").await?;

        let remaining = db.read_all().await?;
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].name, "other_pkg");
        Ok(())
    }
```

- [ ] **Step 3: Run the new tests, confirm they fail**

Run: `cargo test --package DPM --test db_tests`
Expected: compile error (`explicit` field exists on `DbPackage` but `db.rs` doesn't select/insert/decode it yet — `row_to_package`'s struct literal is missing the `explicit` field), or once you've temporarily patched `row_to_package` to compile (see note below), the new assertions fail because `explicit` isn't actually persisted.

Note: after Step 1, `crates/dpm/src/utils/db.rs`'s `row_to_package` function (`fn row_to_package(row: turso::Row) -> ClientResult<DbPackage>`) will fail to compile because its `Ok(DbPackage { ... })` struct literal is missing the new `explicit` field. Add a temporary `explicit: true,` line to that struct literal now, just so the crate compiles and you can observe the new tests fail on their actual assertions rather than on a compile error. Step 4 replaces this with the real implementation.

- [ ] **Step 4: Implement schema + upsert + row decode split**

In `crates/dpm/src/utils/db.rs`, update `run_migrations`'s `is_info == false` branch (the `InstalledPackages`/`installed_files` creation):

```rust
        } else {
            conn.execute(
                r#"CREATE TABLE IF NOT EXISTS InstalledPackages (
                    source TEXT NOT NULL,
                    name TEXT NOT NULL,
                    version TEXT NOT NULL,
                    kind TEXT NOT NULL,
                    url TEXT,
                    hash TEXT,
                    filename TEXT,
                    build_command TEXT,
                    description TEXT NOT NULL,
                    entry TEXT,
                    dependencies TEXT,
                    author TEXT,
                    signature TEXT,
                    explicit INTEGER NOT NULL DEFAULT 1,
                    installed_at DATETIME DEFAULT CURRENT_TIMESTAMP,
                    PRIMARY KEY (name)
                );"#,
                (),
            )
            .await
            .map_err(|e| ClientError::Core(DatabaseError(e.to_string())))?;
            // `CREATE TABLE IF NOT EXISTS` above is a no-op on a DB file that
            // already has `InstalledPackages` from before this column
            // existed, so it wouldn't add `explicit` to it. This `ALTER
            // TABLE` covers that upgrade path; on a brand-new DB (or one
            // already migrated) the column already exists, so SQLite/turso
            // reports "duplicate column name", which we treat as success.
            if let Err(e) = conn
                .execute(
                    "ALTER TABLE InstalledPackages ADD COLUMN explicit INTEGER NOT NULL DEFAULT 1",
                    (),
                )
                .await
            {
                let msg = e.to_string();
                if !msg.to_lowercase().contains("duplicate column") {
                    return Err(ClientError::Core(DatabaseError(msg)));
                }
            }
            conn.execute(
                r#"CREATE TABLE IF NOT EXISTS installed_files (
                    package_name TEXT NOT NULL,
                    file_path TEXT NOT NULL,
                    PRIMARY KEY (package_name, file_path)
                );"#,
                (),
            )
            .await
            .map_err(|e| ClientError::Core(DatabaseError(e.to_string())))?;
        }
        Ok(())
    }
```

Split `row_to_package` into a shared decoder plus two thin wrappers (replace the existing `fn row_to_package` entirely):

```rust
    fn decode_common_fields(row: &turso::Row) -> ClientResult<DbPackage> {
        // Index derived from `COLUMNS` itself (not a hand-copied number), so
        // reordering the column list here can't silently desync the query
        // string from the decode below — turso's `Row` carries no column
        // names of its own, only `Rows` does, so this is the closest we can
        // get to name-based lookup without threading `Rows` through.
        let col_idx = |name: &str| -> ClientResult<usize> {
            COLUMNS
                .split(", ")
                .position(|c| c == name)
                .ok_or_else(|| ClientError::Core(DatabaseError(format!("no column {name}"))))
        };
        let get_text = |name: &str| -> ClientResult<String> {
            row.get_value(col_idx(name)?)
                .map_err(|e| ClientError::Core(DatabaseError(e.to_string())))?
                .as_text()
                .cloned()
                .ok_or_else(|| {
                    ClientError::Core(DatabaseError(format!("column {name} is not text")))
                })
        };
        let get_opt_text = |name: &str| -> ClientResult<Option<String>> {
            Ok(row
                .get_value(col_idx(name)?)
                .map_err(|e| ClientError::Core(DatabaseError(e.to_string())))?
                .as_text()
                .cloned())
        };
        let dependencies_json = get_opt_text("dependencies")?;
        Ok(DbPackage {
            source: get_text("source")?,
            name: get_text("name")?,
            version: get_text("version")?,
            kind: get_text("kind")?,
            url: get_opt_text("url")?,
            hash: get_opt_text("hash")?,
            filename: get_opt_text("filename")?,
            build_command: get_opt_text("build_command")?,
            description: get_text("description")?,
            entry: get_opt_text("entry")?,
            dependencies: dependencies_json.and_then(|json| serde_json::from_str(&json).ok()),
            author: get_opt_text("author")?,
            signature: get_opt_text("signature")?,
            // Meaningless for `AvailablePackages` rows (no such column there);
            // `row_to_installed_package` below overwrites this with the real
            // value for `InstalledPackages` rows.
            explicit: true,
        })
    }

    fn row_to_available_package(row: turso::Row) -> ClientResult<DbPackage> {
        Self::decode_common_fields(&row)
    }

    fn row_to_installed_package(row: turso::Row) -> ClientResult<DbPackage> {
        let mut pkg = Self::decode_common_fields(&row)?;
        // `explicit` isn't part of the shared `COLUMNS` list (it doesn't
        // exist on `AvailablePackages`), so it's always the column
        // immediately after everything `COLUMNS` lists — callers select it
        // as `SELECT {COLUMNS}, explicit FROM InstalledPackages`.
        let explicit_idx = COLUMNS.split(", ").count();
        let explicit_value = row
            .get_value(explicit_idx)
            .map_err(|e| ClientError::Core(DatabaseError(e.to_string())))?
            .as_integer()
            .copied()
            .ok_or_else(|| {
                ClientError::Core(DatabaseError("column explicit is not an integer".to_string()))
            })?;
        pkg.explicit = explicit_value != 0;
        Ok(pkg)
    }
```

Update every caller of the old `row_to_package`:
- `read_available` (`SELECT {COLUMNS} FROM AvailablePackages`) → `Self::row_to_available_package(row)?`
- `read_one_available` → `Self::row_to_available_package(row)?`
- `search_available` → `Self::row_to_available_package(row)?`
- `versions_of` (`SELECT {COLUMNS} FROM AvailablePackages WHERE ...`) → `Self::row_to_available_package(row)?`
- `latest_version` (`SELECT {COLUMNS} FROM AvailablePackages WHERE ...`) → `Self::row_to_available_package(row)?`
- `read_all`: change the query to `&format!("SELECT {COLUMNS}, explicit FROM InstalledPackages")` and the decode call to `Self::row_to_installed_package(row)?`
- `read_one`: change the query to `&format!("SELECT {COLUMNS}, explicit FROM InstalledPackages WHERE source = ?1 AND name = ?2 AND version = ?3")` and the decode call to `Self::row_to_installed_package(row)?`

Replace `insert`'s body with an upsert that writes `explicit` and never lets a conflicting row's `explicit` regress from `true` to `false`:

```rust
    pub async fn insert(&self, pkg: DbPackage) -> ClientResult<()> {
        let dependencies_json = pkg
            .dependencies
            .as_ref()
            .map(|deps| serde_json::to_string(deps).unwrap_or_else(|_| "[]".to_string()));
        let conn = self.connect().await?;
        let to_value = |opt: Option<String>| match opt {
            Some(s) => turso::Value::Text(s),
            None => turso::Value::Null,
        };
        let params: Vec<turso::Value> = vec![
            turso::Value::Text(pkg.source),
            turso::Value::Text(pkg.name),
            turso::Value::Text(pkg.version),
            turso::Value::Text(pkg.kind),
            to_value(pkg.url),
            to_value(pkg.hash),
            to_value(pkg.filename),
            to_value(pkg.build_command),
            turso::Value::Text(pkg.description),
            to_value(pkg.entry),
            to_value(dependencies_json),
            to_value(pkg.author),
            to_value(pkg.signature),
            turso::Value::Integer(if pkg.explicit { 1 } else { 0 }),
        ];
        conn.execute(
            &format!(
                "INSERT INTO InstalledPackages ({COLUMNS}, explicit) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14) \
                 ON CONFLICT(name) DO UPDATE SET \
                    source = excluded.source, \
                    version = excluded.version, \
                    kind = excluded.kind, \
                    url = excluded.url, \
                    hash = excluded.hash, \
                    filename = excluded.filename, \
                    build_command = excluded.build_command, \
                    description = excluded.description, \
                    entry = excluded.entry, \
                    dependencies = excluded.dependencies, \
                    author = excluded.author, \
                    signature = excluded.signature, \
                    explicit = CASE WHEN InstalledPackages.explicit = 1 THEN 1 ELSE excluded.explicit END"
            ),
            params,
        )
        .await
        .map_err(|e| ClientError::Core(DatabaseError(e.to_string())))?;
        Ok(())
    }
```

Add `delete_installed` right after `insert` (this is the parameterized replacement for `uninstall()`'s current string-formatted `DELETE`, wired up in Task 4):

```rust
    /// Parameterized delete for a single `InstalledPackages` row by name —
    /// used by `uninstall()`/`autoremove()` instead of hand-formatting SQL
    /// with the package name spliced directly into the query string.
    pub async fn delete_installed(&self, name: &str) -> ClientResult<()> {
        let conn = self.connect().await?;
        conn.execute("DELETE FROM InstalledPackages WHERE name = ?1", [name])
            .await
            .map_err(|e| ClientError::Core(DatabaseError(e.to_string())))?;
        Ok(())
    }
```

- [ ] **Step 5: Run the tests, confirm they pass**

Run: `cargo test --package DPM --test db_tests`
Expected: all tests in `db_tests.rs` (existing ones + the four new ones from Step 2) PASS.

- [ ] **Step 6: Full workspace check and commit**

Run: `cargo check --workspace && cargo clippy --workspace --all-targets -- -D warnings`
Expected: no errors, no warnings (fix any `DbPackage::new` call site Step 1's grep missed).

```bash
git add crates/dpm/src/utils/models.rs crates/dpm/src/utils/db.rs crates/dpm/src/action.rs crates/dpm/src/context.rs crates/dpm/src/utils/resolver.rs crates/dpm/src/utils/fetcher.rs crates/dpm/tests/db_tests.rs
git commit -m "feat(dpm): add explicit/auto flag to InstalledPackages with sticky-promote upsert"
```

---

### Task 2: Orphan detection (`find_orphans`)

**Files:**
- Create: `crates/dpm/src/utils/orphan.rs`
- Modify: `crates/dpm/src/utils/mod.rs` (register the new module)

**Interfaces:**
- Consumes: `DbPackage` (`crate::DbPackage`, from Task 1 — has `.explicit: bool` and `.dependencies: Option<Vec<dpm_core::Dependency>>`, where `Dependency { name: String, version: String }`).
- Produces: `pub fn find_orphans(installed: &[DbPackage]) -> Vec<DbPackage>` — used by Task 4 (`uninstall()`'s hint) and Task 5 (`autoremove()`).

- [ ] **Step 1: Write the failing tests**

Create `crates/dpm/src/utils/orphan.rs`:

```rust
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
            "official", name, "1.0.0", "prebuilt", None, None, None, None, "", None,
            dependencies, None, None, explicit,
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
```

- [ ] **Step 2: Wire the module in and run the tests**

In `crates/dpm/src/utils/mod.rs`, add the module declaration and re-export (alphabetically between `models` and `placer`):

```rust
pub mod db;
pub mod download;
pub mod error;
pub mod fetcher;
pub mod models;
pub mod orphan;
pub mod placer;
pub mod privilege;
pub mod resolver;
pub mod source_clone;
pub mod system;
pub use self::db::*;
pub use self::download::*;
pub use self::error::*;
pub use self::fetcher::*;
pub use self::models::*;
pub use self::orphan::*;
pub use self::placer::*;
pub use self::resolver::*;
pub use self::source_clone::*;
pub use self::system::*;
pub use dpm_core::{read_file_from_zip, unzip_file, zip_folder};
```

Run: `cargo test --package DPM orphan::`
Expected: all 5 tests in `crates/dpm/src/utils/orphan.rs` PASS (this is a pure function with no I/O, so there's no separate "fails first" step worth doing here beyond the normal compile-then-pass cycle — write it once, correctly, per the spec's fixpoint algorithm above).

- [ ] **Step 3: Commit**

```bash
git add crates/dpm/src/utils/orphan.rs crates/dpm/src/utils/mod.rs
git commit -m "feat(dpm): add find_orphans for detecting unreferenced auto-installed packages"
```

---

### Task 3: Wire `explicit` into the install path

**Files:**
- Modify: `crates/dpm/src/action.rs` (`install_resolved_with_gate`, `install_source_package`)

**Interfaces:**
- Consumes: `ParsedInstallSpec = (Option<String>, String, Option<String>)` (already defined in `action.rs`), `DbPackage.explicit` (Task 1).
- Produces: `fn is_directly_requested(is: &[ParsedInstallSpec], name: &str) -> bool` (private, used only within this file).

- [ ] **Step 1: Write the failing test**

Add near the bottom of `action.rs`'s `#[cfg(test)] mod tests` block (alongside the other small helper tests):

```rust
    #[test]
    fn is_directly_requested_matches_by_name_only() {
        let is: Vec<ParsedInstallSpec> =
            vec![(Some("official".to_string()), "foo".to_string(), None)];
        assert!(is_directly_requested(&is, "foo"));
        assert!(!is_directly_requested(&is, "bar"));
    }
```

- [ ] **Step 2: Run it, confirm it fails**

Run: `cargo test --package DPM is_directly_requested`
Expected: compile error — `is_directly_requested` is not defined yet.

- [ ] **Step 3: Implement `is_directly_requested` and wire it into both install paths**

In `crates/dpm/src/action.rs`, add this function near `install_resolved_with_gate` (e.g. directly above it):

```rust
    /// Whether `name` was directly requested on this command's `is` list —
    /// i.e. deserves `explicit = true` when written to `InstalledPackages`,
    /// as opposed to being pulled in only because another package's
    /// `dependencies` resolved to it. `is` only ever contains what the user
    /// actually typed (see `parse_mine`), never the transitive closure
    /// `resolve_install_set` adds on top.
    fn is_directly_requested(is: &[ParsedInstallSpec], name: &str) -> bool {
        is.iter().any(|(_, n, _)| n == name)
    }
```

In `install_resolved_with_gate`, right after `let pkg = name.as_str();` (inside the `for (source_alias, name, version) in resolved` loop), compute the flag once:

```rust
                let pkg = name.as_str();
                let explicit = Self::is_directly_requested(is, pkg);
```

Change the `Source`-kind branch's call from:
```rust
                if matches!(repo_package_info.kind()?, PackageKind::Source { .. }) {
                    self.install_source_package(pkg, &source_alias, repo_package_info, &staging)
                        .await?;
```
to:
```rust
                if matches!(repo_package_info.kind()?, PackageKind::Source { .. }) {
                    self.install_source_package(
                        pkg,
                        &source_alias,
                        repo_package_info,
                        explicit,
                        &staging,
                    )
                    .await?;
```

Change the prebuilt-path insert (currently `self.ctx.db.insert(repo_package_info.clone()).await?;`, right after the `place_package(...)` call) to:
```rust
                let mut installed_pkg = repo_package_info.clone();
                installed_pkg.explicit = explicit;
                self.ctx.db.insert(installed_pkg).await?;
```

Update `install_source_package`'s signature to accept the new parameter:
```rust
    async fn install_source_package(
        &self,
        pkg: &str,
        source_alias: &str,
        repo_package_info: &DbPackage,
        explicit: bool,
        staging: &tempfile::TempDir,
    ) -> ClientResult<()> {
```

And change its own insert (currently `self.ctx.db.insert(repo_package_info.clone()).await?;`, right after its `place_package(...)` call) to:
```rust
        let mut installed_pkg = repo_package_info.clone();
        installed_pkg.explicit = explicit;
        self.ctx.db.insert(installed_pkg).await?;
```

- [ ] **Step 4: Run the test, confirm it passes**

Run: `cargo test --package DPM is_directly_requested`
Expected: PASS.

- [ ] **Step 5: Full workspace check**

Run: `cargo check --workspace && cargo clippy --workspace --all-targets -- -D warnings && cargo test --package DPM`
Expected: no errors, no warnings, all existing `action.rs` tests still pass (the signature-verification tests in Task 1's grep list call `install_resolved_with_gate` directly and don't reach the `db.insert` line, so they're unaffected by this change — confirm this by reading their assertions if any fail unexpectedly).

Note on scope: this task deliberately does not add a full successful-install integration test — no existing test in `action.rs` completes a real download+unzip+place (they all assert on the signature gate or a network failure past it, per the project's own convention of leaving real end-to-end verification to manual testing, see `docs/superpowers/plans/2026-07-27-*.md`'s "手動端到端驗證" sections). Task 1's DB-layer tests already prove `explicit` persists correctly once a `DbPackage` carrying the right value reaches `db.insert()`; this task's job is only to prove that value is computed correctly before it gets there, which `is_directly_requested`'s unit test does directly. Full-loop confirmation is Task 6's manual verification step.

- [ ] **Step 6: Commit**

```bash
git add crates/dpm/src/action.rs
git commit -m "feat(dpm): compute and persist explicit flag on install"
```

---

### Task 4: `uninstall()` refactor — shared removal helper, parameterized delete, orphan hint

**Files:**
- Modify: `crates/dpm/src/action.rs` (`uninstall`, new `remove_installed_package` helper)

**Interfaces:**
- Consumes: `find_orphans` (Task 2), `Db::delete_installed` (Task 1).
- Produces: `async fn remove_installed_package(&self, pkg: &str) -> ClientResult<()>` (private method on `ActionInfo`, used by `uninstall()` here and by `autoremove()` in Task 5).

- [ ] **Step 1: Write the failing test**

Add to `action.rs`'s test module:

```rust
    #[tokio::test]
    async fn uninstall_prints_hint_and_leaves_orphan_installed_for_autoremove_to_handle()
    -> ClientResult<()> {
        let root = tempfile::tempdir().unwrap();
        let ctx = Context::for_test(root.path()).await.unwrap();

        let leaf = DbPackage::new(
            "official", "leaf", "1.0.0", "prebuilt", None, None, None, None, "", None, None,
            None, None, false,
        );
        ctx.db.insert(leaf).await.unwrap();

        let root_pkg = DbPackage::new(
            "official", "root", "1.0.0", "prebuilt", None, None, None, None, "", None,
            Some(vec![dpm_core::Dependency {
                name: "leaf".to_string(),
                version: "*".to_string(),
            }]),
            None, None, true,
        );
        ctx.db.insert(root_pkg).await.unwrap();

        let action = ActionInfo::new(ctx.clone(), vec!["root".to_string()], false, Setting::default());
        action.uninstall().await?;

        // "root" is gone, "leaf" is still installed (autoremove's job, not
        // uninstall's) but now orphaned.
        let remaining = ctx.db.read_all().await.unwrap();
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].name, "leaf");
        assert!(!remaining[0].explicit);
        assert!(find_orphans(&remaining).iter().any(|p| p.name == "leaf"));
        Ok(())
    }
```

- [ ] **Step 2: Run it, confirm it fails**

Run: `cargo test --package DPM uninstall_prints_hint_and_leaves_orphan_installed`
Expected: FAIL — either a compile error (if `DbPackage::new`'s `dependencies` param type mismatch trips first) or, once that's sorted, the test still passes today's `uninstall()` behavior differently only in that today's code already deletes "root" the same way; this test's real job is to lock in the *coexisting* Task 2 behavior, so if it already passes at this point that's fine — the important assertions to watch are compiled and green after Step 3, not necessarily red before it. Proceed to Step 3 regardless and re-run.

- [ ] **Step 3: Implement the shared helper, refactor `uninstall()`, add the hint**

In `crates/dpm/src/action.rs`, replace the entire body of `uninstall()`:

```rust
    pub async fn uninstall(&self) -> ClientResult<()> {
        let (_, is, isnot) = self.parsed_packages(false).await?;
        if is.is_empty() && isnot.is_empty() {
            println!("{}", "No matching installed packages found to uninstall.".yellow());
            return Ok(());
        }
        if !is.is_empty() {
            for (_, pkg, _) in is {
                if self.verbose {
                    println!("{}\n\n  {}", pkg.on_green(), "Removing...".red());
                }
                self.remove_installed_package(&pkg).await?;
                if self.verbose {
                    println!("  {}", "Done".green());
                }
            }
        }
        if !isnot.is_empty() {
            for pkg in isnot {
                println!("==> Host OS Package Manager ({}): Removing '{pkg}'...", self.system_action.primary_manager_name().cyan());
                self.system_action.uninstall_package(&pkg)?;
            }
        }

        let remaining = self
            .ctx
            .db
            .read_all()
            .await
            .map_err(|e| ClientError::Core(CoreError::DatabaseError(e.to_string())))?;
        let orphans = find_orphans(&remaining);
        if !orphans.is_empty() {
            let names: Vec<&str> = orphans.iter().map(|p| p.name.as_str()).collect();
            println!(
                "{} {} package(s) are now orphaned: {}. Run 'dpm autoremove' to remove them.",
                "Note:".yellow(),
                orphans.len(),
                names.join(", ")
            );
        }
        Ok(())
    }

    /// Removes `pkg`'s on-disk files (per the `installed_files` manifest,
    /// plus the install-dir/opt-link/bin-link fallback paths it also cleans
    /// up) and its `InstalledPackages`/`installed_files` DB rows. Shared by
    /// `uninstall()` and `autoremove()` so there is exactly one place that
    /// knows how to fully remove an installed package.
    async fn remove_installed_package(&self, pkg: &str) -> ClientResult<()> {
        let pre_rm_location = self.ctx.install_dir.join(pkg);
        let opt_link = self.ctx.opt_dir().join(pkg);

        // 1. O(1) DB Manifest Cleanup: remove every file/symlink registered in DB
        let recorded_files = self
            .ctx
            .db
            .get_installed_files(pkg)
            .await
            .unwrap_or_default();
        for file_path in recorded_files {
            let p = Path::new(&file_path);
            if p.exists() || p.symlink_metadata().is_ok() {
                let _ = remove_file(p).or_else(|_| remove_dir_all(p));
            }
        }

        // 2. Remove main install dir Software/<pkg> & opt/<pkg>
        if opt_link.exists() || opt_link.symlink_metadata().is_ok() {
            let _ = remove_file(&opt_link);
        }
        if pre_rm_location.exists() {
            let _ = remove_dir_all(&pre_rm_location);
        }

        // 3. Fallback cleanup for direct bin link if present
        let direct_bin_link = self.ctx.bin_dir.join(pkg);
        if direct_bin_link.exists() || direct_bin_link.symlink_metadata().is_ok() {
            let _ = remove_file(&direct_bin_link);
        }

        // 4. Remove manifest rows from DB
        self.ctx.db.remove_installed_files(pkg).await?;
        self.ctx.db.delete_installed(pkg).await?;
        Ok(())
    }
```

This removes the old `self.ctx.db.execute_query(&format!("DELETE FROM InstalledPackages WHERE name = '{}'", pkg)).await?;` line entirely (replaced by `self.ctx.db.delete_installed(pkg).await?;` inside the new helper) — that was the unparameterized string-formatted DELETE Task 1's `delete_installed` exists to replace.

Make sure `find_orphans` is in scope — it already will be, since `crates/dpm/src/action.rs` imports via `use crate::{... system::*, ...}` and `find_orphans` is re-exported from `crate::utils` (Task 2) into the crate root via `lib.rs`'s `pub use utils::*;`. If `cargo check` reports it's not found, add `find_orphans` explicitly to the existing `use crate::{...}` block at the top of `action.rs`.

- [ ] **Step 4: Run the test, confirm it passes**

Run: `cargo test --package DPM uninstall_prints_hint_and_leaves_orphan_installed`
Expected: PASS.

- [ ] **Step 5: Run the full existing `uninstall`-adjacent test suite**

Run: `cargo test --package DPM`
Expected: all tests pass, including any pre-existing tests that exercised `uninstall()`'s old file-removal behavior (behavior is unchanged, only extracted into a helper + the DELETE is now parameterized).

- [ ] **Step 6: Commit**

```bash
git add crates/dpm/src/action.rs
git commit -m "refactor(dpm): extract remove_installed_package, parameterize uninstall's DELETE, print orphan hint"
```

---

### Task 5: `dpm autoremove` CLI command

**Files:**
- Modify: `crates/dpm/src/cli_parse.rs` (new `Commands::Autoremove` variant)
- Modify: `crates/dpm/src/lib.rs` (dispatch arm)
- Modify: `crates/dpm/src/action.rs` (new `autoremove()` method)

**Interfaces:**
- Consumes: `find_orphans` (Task 2), `ActionInfo::remove_installed_package` (Task 4).
- Produces: `pub async fn autoremove(&self) -> ClientResult<()>` on `ActionInfo`; `Commands::Autoremove { verbose: bool }` CLI variant with aliases `ar`/`auto`.

- [ ] **Step 1: Write the failing test**

Add to `action.rs`'s test module:

```rust
    #[tokio::test]
    async fn autoremove_removes_orphans_and_leaves_explicit_packages_alone() -> ClientResult<()> {
        let root = tempfile::tempdir().unwrap();
        let ctx = Context::for_test(root.path()).await.unwrap();

        let orphan = DbPackage::new(
            "official", "orphan", "1.0.0", "prebuilt", None, None, None, None, "", None, None,
            None, None, false,
        );
        ctx.db.insert(orphan).await.unwrap();

        let kept = DbPackage::new(
            "official", "kept", "1.0.0", "prebuilt", None, None, None, None, "", None, None,
            None, None, true,
        );
        ctx.db.insert(kept).await.unwrap();

        let action = ActionInfo::new(ctx.clone(), vec![], false, Setting::default());
        action.autoremove().await?;

        let remaining = ctx.db.read_all().await.unwrap();
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].name, "kept");
        Ok(())
    }

    #[tokio::test]
    async fn autoremove_is_a_no_op_when_nothing_is_orphaned() -> ClientResult<()> {
        let root = tempfile::tempdir().unwrap();
        let ctx = Context::for_test(root.path()).await.unwrap();

        let kept = DbPackage::new(
            "official", "kept", "1.0.0", "prebuilt", None, None, None, None, "", None, None,
            None, None, true,
        );
        ctx.db.insert(kept).await.unwrap();

        let action = ActionInfo::new(ctx.clone(), vec![], false, Setting::default());
        action.autoremove().await?;

        assert_eq!(ctx.db.read_all().await.unwrap().len(), 1);
        Ok(())
    }
```

- [ ] **Step 2: Run it, confirm it fails**

Run: `cargo test --package DPM autoremove`
Expected: compile error — `autoremove` method doesn't exist on `ActionInfo` yet.

- [ ] **Step 3: Implement `ActionInfo::autoremove`**

In `crates/dpm/src/action.rs`, add near `uninstall()`:

```rust
    pub async fn autoremove(&self) -> ClientResult<()> {
        let installed = self
            .ctx
            .db
            .read_all()
            .await
            .map_err(|e| ClientError::Core(CoreError::DatabaseError(e.to_string())))?;
        let orphans = find_orphans(&installed);
        if orphans.is_empty() {
            println!("{}", "No orphaned packages found.".yellow());
            return Ok(());
        }

        println!("{}", "==> Orphaned packages:".green().bold());
        for pkg in &orphans {
            println!("  {} v{}", pkg.name.bold(), pkg.version);
        }
        for pkg in &orphans {
            if self.verbose {
                println!("{}\n\n  {}", pkg.name.as_str().on_green(), "Removing...".red());
            }
            self.remove_installed_package(&pkg.name).await?;
        }
        println!("{} {} package(s) removed.", "Done:".green(), orphans.len());
        Ok(())
    }
```

- [ ] **Step 4: Run the tests, confirm they pass**

Run: `cargo test --package DPM autoremove`
Expected: PASS.

- [ ] **Step 5: Add the CLI command**

In `crates/dpm/src/cli_parse.rs`, add a new variant to `Commands` right after `UpgradeSelf` and before `Source`:

```rust
    /// Remove orphaned dependencies (installed automatically, no longer needed)
    #[command(visible_aliases = ["ar", "auto"])]
    Autoremove {
        #[arg(short, long)]
        verbose: bool,
    },
```

In `crates/dpm/src/lib.rs`, add the matching dispatch arm right after the `Commands::UpgradeSelf` arm and before `Commands::Source`:

```rust
        Some(Commands::Autoremove { verbose }) => {
            ActionInfo::new(ctx.clone(), vec![], verbose, setting_config)
                .autoremove()
                .await?
        }
```

- [ ] **Step 6: Full workspace check**

Run: `cargo check --workspace && cargo clippy --workspace --all-targets -- -D warnings && cargo test --package DPM`
Expected: no errors, no warnings, all tests pass.

- [ ] **Step 7: Commit**

```bash
git add crates/dpm/src/cli_parse.rs crates/dpm/src/lib.rs crates/dpm/src/action.rs
git commit -m "feat(dpm): add dpm autoremove command"
```

---

### Task 6: Workspace-wide verification and TODO.md

**Files:** No new source changes — verification + docs only.
- Modify: `TODO.md` (check off the autoremove item)

**Interfaces:** None (this task consumes everything built in Tasks 1-5).

- [ ] **Step 1: Full workspace check**

Run: `cargo check --workspace`
Expected: compiles cleanly.

- [ ] **Step 2: Format check**

Run: `cargo fmt --all -- --check`
Expected: no output. If there is output, run `cargo fmt --all` and re-check.

- [ ] **Step 3: Clippy**

Run: `cargo clippy --workspace --all-targets -- -D warnings`
Expected: no warnings, no errors.

- [ ] **Step 4: Full test suite**

Run: `cargo test --workspace`
Expected: all tests pass, including every test added in Tasks 1-5.

- [ ] **Step 5: Manual end-to-end verification**

This is the check Task 3 deferred (no existing test in this codebase completes a real successful install). Using a local build (`just run-client install ...` or `cargo run -p dpm -- install ...` against a real or local `file://` source with at least one package that has a `dependencies` entry):

1. `dpm install <pkg-with-a-dependency>` — confirm both the package and its dependency show up installed.
2. `dpm uninstall <pkg-with-a-dependency>` — confirm the dependency is still installed but the CLI prints the `"N package(s) are now orphaned"` hint naming it.
3. `dpm autoremove` — confirm it lists and removes exactly that dependency, and prints the removal summary.
4. `dpm autoremove` again — confirm it prints `"No orphaned packages found."` and does nothing.
5. `dpm install <the-dependency-by-name-directly>`, then remove whatever package would otherwise orphan it, then `dpm autoremove` — confirm it is **not** removed (promoted to explicit).

Write a one-line note in the PR/commit description confirming this manual pass succeeded.

- [ ] **Step 6: Update TODO.md**

In `TODO.md`, find the line:
```
- [ ] **Autoremove / orphan 清理** — 沒有。裝了套件當某東西的依賴,那東西被移除後不會自動變孤兒清單,也沒有指令能一次清掉。優先做這個,套件管理器基本盤。
```
and change it to:
```
- [x] **Autoremove / orphan 清理** — 已實作。`InstalledPackages` 新增 `explicit` 欄位區分主動安裝 vs 依賴拉進來,`find_orphans`(`crates/dpm/src/utils/orphan.rs`)遞迴找出不再被引用的 auto 套件,`dpm autoremove` 指令清除,`dpm uninstall` 收尾會印孤兒提示。設計見 `docs/superpowers/specs/2026-08-06-autoremove-design.md`。
```

- [ ] **Step 7: Commit**

```bash
git add TODO.md
git commit -m "docs: mark autoremove feature gap as done in TODO.md"
```

## Verification Checklist

- [ ] `cargo check --workspace` passes
- [ ] `cargo clippy --workspace --all-targets -- -D warnings` passes
- [ ] `cargo test --workspace` passes, including all new tests from Tasks 1-5
- [ ] Manual end-to-end verification (Task 6 Step 5) completed
- [ ] `TODO.md`'s autoremove item checked off
