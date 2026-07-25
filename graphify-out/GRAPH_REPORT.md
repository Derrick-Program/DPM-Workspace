# Graph Report - .  (2026-07-25)

## Corpus Check
- Corpus is ~39,248 words - fits in a single context window. You may not need a graph.

## Summary
- 439 nodes · 854 edges · 26 communities (24 shown, 2 thin omitted)
- Extraction: 97% EXTRACTED · 3% INFERRED · 0% AMBIGUOUS · INFERRED: 26 edges (avg confidence: 0.77)
- Token cost: 0 input · 247,630 output

## Community Hubs (Navigation)
- [[_COMMUNITY_Project Docs & Design Rationale|Project Docs & Design Rationale]]
- [[_COMMUNITY_Pubgrub Constraint Parsing|Pubgrub Constraint Parsing]]
- [[_COMMUNITY_Client Install Action Logic|Client Install Action Logic]]
- [[_COMMUNITY_Local Turso DB Layer|Local Turso DB Layer]]
- [[_COMMUNITY_Core Package Data Model|Core Package Data Model]]
- [[_COMMUNITY_Server Publish CLI Actions|Server Publish CLI Actions]]
- [[_COMMUNITY_Legacy JSON Package Storage|Legacy JSON Package Storage]]
- [[_COMMUNITY_OS Package Manager Fallback|OS Package Manager Fallback]]
- [[_COMMUNITY_Client CLI Arg Parsing|Client CLI Arg Parsing]]
- [[_COMMUNITY_Server Versioning Test Suite|Server Versioning Test Suite]]
- [[_COMMUNITY_Client CLI Parse Tests|Client CLI Parse Tests]]
- [[_COMMUNITY_Client DB Test Suite|Client DB Test Suite]]
- [[_COMMUNITY_Zip File Utilities|Zip File Utilities]]
- [[_COMMUNITY_Error Type Definitions|Error Type Definitions]]
- [[_COMMUNITY_Source Repo Clone Utility|Source Repo Clone Utility]]
- [[_COMMUNITY_Test Fixture helloWorld Package|Test Fixture: helloWorld Package]]
- [[_COMMUNITY_Test Fixture test Package|Test Fixture: test Package]]
- [[_COMMUNITY_Test Fixture test1 Package|Test Fixture: test1 Package]]
- [[_COMMUNITY_Test Fixture test2 Package|Test Fixture: test2 Package]]
- [[_COMMUNITY_Client Entry Point|Client Entry Point]]
- [[_COMMUNITY_Test Fixture Shell Script|Test Fixture Shell Script]]

## God Nodes (most connected - your core abstractions)
1. `Db` - 21 edges
2. `DbPackage` - 20 edges
3. `ActionInfo` - 19 edges
4. `resolve_install_set()` - 19 edges
5. `TODO.md workspace scan findings` - 16 edges
6. `RepoInfo` - 14 edges
7. `setup_db()` - 14 edges
8. `DPM-Workspace CLAUDE.md project documentation` - 14 edges
9. `Multi-source registry ecosystem design (namespace+versioning+pubgrub+publish+CI+security)` - 14 edges
10. `PackageVersionInfo` - 13 edges

## Surprising Connections (you probably didn't know these)
- `dpm-server README` --semantically_similar_to--> `dpm-server no-binary-hosting publish model (fix add --url/--build, packages/ rename)`  [INFERRED] [semantically similar]
  crates/dpm-server/README.md → docs/superpowers/plans/2026-07-24-server-publish-model.md
- `Legacy per-crate Rust build/test CI (pre-workspace-merge)` --conceptually_related_to--> `DPM-Workspace CLAUDE.md project documentation`  [INFERRED]
  crates/dpm/.github/workflows/rust.yml → CLAUDE.md
- `test_hash_file_is_deterministic_and_content_sensitive()` --calls--> `hash_file()`  [INFERRED]
  crates/dpm-core/tests/test.rs → crates/dpm-core/src/lib.rs
- `PR Check: verify-index job` --conceptually_related_to--> `Legacy per-crate Rust build/test CI (pre-workspace-merge)`  [INFERRED]
  .github/workflows/pr-check.yml → crates/dpm/.github/workflows/rust.yml
- `TODO.md workspace scan findings` --references--> `dpm client README (minimal placeholder)`  [EXTRACTED]
  TODO.md → crates/dpm/README.md

## Import Cycles
- 1-file cycle: `crates/dpm/src/utils/error.rs -> crates/dpm/src/utils/error.rs`

## Hyperedges (group relationships)
- **Five-phase implementation of the multi-source registry design spec** — docs_superpowers_specs_2026_07_24_multi_source_registry_design, docs_superpowers_plans_2026_07_24_blake3_tempfile_atomic_install, docs_superpowers_plans_2026_07_24_multi_source_namespace, docs_superpowers_plans_2026_07_24_server_publish_model, docs_superpowers_plans_2026_07_25_client_source_install, docs_superpowers_plans_2026_07_25_pubgrub_dependency_resolution [EXTRACTED 1.00]
- **CI/CD package-index verification pipeline (pr-check + publish sanity check)** — github_workflows_pr_check_verify_index, github_workflows_publish_sanity_check, concept_ci_pr_check_workflow, concept_dpm_server_publish_model [EXTRACTED 1.00]
- **TODO.md debt items closed out by later implementation plans** — todo_config_not_persisted_bug, todo_unused_deps_git2, todo_readme_stale_example, concept_source_config_schema, concept_client_source_install [INFERRED 0.85]

## Communities (26 total, 2 thin omitted)

### Community 0 - "Project Docs & Design Rationale"
Cohesion: 0.06
Nodes (62): CLAUDE.md: dpm hand-rolled clap::Command vs dpm-server derive(Parser), CLAUDE.md: Client 資料層 (turso+geni) architecture bullet, DPM-Workspace CLAUDE.md project documentation, CLAUDE.md: dual-scope permission model bullet, CLAUDE.md: Server 資料 (packages/, hashes.json, packageInfo.json, RepoInfo.json, init/hash/build/fix), tempfile staging + atomic rename install (swap_into_install_dir), Shared dpm-core blake3 hash_file(), CI package-index verification workflows (pr-check.yml / publish.yml) (+54 more)

### Community 1 - "Pubgrub Constraint Parsing"
Cohesion: 0.08
Nodes (35): Comparator, ambiguous_dependency_name_errors(), ambiguous_top_level_request_errors(), caret_normal(), caret_zero_major(), caret_zero_zero(), comparator_range(), comparator_to_range() (+27 more)

### Community 2 - "Client Install Action Logic"
Cohesion: 0.11
Nodes (23): ActionInfo, chown_dir_to_sudo_user(), drop_privileges_for_build(), entry_is_safe(), entry_resolves_inside_install_dir(), fresh_install_moves_new_dir_into_place(), old_install_survives_if_new_dir_is_missing(), rejects_symlink_escaping_install_dir() (+15 more)

### Community 3 - "Local Turso DB Layer"
Cohesion: 0.13
Nodes (15): Connection, Db, ClientResult, Option, Path, Self, String, Vec (+7 more)

### Community 4 - "Core Package Data Model"
Cohesion: 0.16
Nodes (17): CoreResult, Dependency, hash_file(), JsonStorage, JsonStorage<T>, PackageInfo, PackageKind, PackageVersionInfo (+9 more)

### Community 5 - "Server Publish CLI Actions"
Cohesion: 0.15
Nodes (24): AnyhowResult, build(), fix(), fix_add(), fix_del(), hash(), init(), Result (+16 more)

### Community 6 - "Legacy JSON Package Storage"
Cohesion: 0.16
Nodes (14): JsonStorage, JsonStorage<T>, PackageBasicInfo, PackageInfo, RepoInfo, HashMap, Option, Path (+6 more)

### Community 7 - "OS Package Manager Fallback"
Cohesion: 0.20
Nodes (8): PackageManager, ClientResult, Option, Self, String, Vec, SystemAction, SystemController

### Community 8 - "Client CLI Arg Parsing"
Cohesion: 0.20
Nodes (15): Cli, CliCommands, Option_set, Option, String, Vec, Scope, SourceAction (+7 more)

### Community 9 - "Server Versioning Test Suite"
Cohesion: 0.16
Nodes (8): add_package_version_appends_and_is_queryable(), add_package_version_keeps_multiple_versions(), add_package_version_rejects_duplicate_version(), prebuilt(), remove_last_package_version_drops_the_package_entirely(), remove_package_version_missing_version_errors(), remove_package_version_removes_only_that_version(), test_hash_file_is_deterministic_and_content_sensitive()

### Community 10 - "Client CLI Parse Tests"
Cohesion: 0.21
Nodes (15): build_cli(), get_args(), get_styles(), print_completions(), Cli, Command, Result, Styles (+7 more)

### Community 11 - "Client DB Test Suite"
Cohesion: 0.32
Nodes (15): Box, Error, Path, Result, sample_pkg(), setup_db(), test_clear_table_for_source_only_wipes_that_source(), test_db_new_and_migrations() (+7 more)

### Community 12 - "Zip File Utilities"
Cohesion: 0.23
Nodes (13): read_file_from_zip(), Path, Result, String, unzip_file(), zip_folder(), read_file_from_zip(), Path (+5 more)

### Community 13 - "Error Type Definitions"
Cohesion: 0.22
Nodes (10): CoreError, Error, String, String, ServerError, ClientError, String, format_pubgrub_error() (+2 more)

### Community 14 - "Source Repo Clone Utility"
Cohesion: 0.39
Nodes (8): clone_package_source(), clones_and_finds_the_package_subdirectory(), make_source_repo(), missing_package_subdirectory_is_an_error(), ClientResult, Path, PathBuf, TempDir

### Community 15 - "Test Fixture: helloWorld Package"
Cohesion: 0.29
Nodes (6): dependencies, description, file_name, hash, package_name, version

### Community 16 - "Test Fixture: test Package"
Cohesion: 0.29
Nodes (6): dependencies, description, file_name, hash, package_name, version

### Community 17 - "Test Fixture: test1 Package"
Cohesion: 0.29
Nodes (6): dependencies, description, file_name, hash, package_name, version

### Community 18 - "Test Fixture: test2 Package"
Cohesion: 0.29
Nodes (6): dependencies, description, file_name, hash, package_name, version

## Knowledge Gaps
- **27 isolated node(s):** `package_name`, `file_name`, `version`, `description`, `hash` (+22 more)
  These have ≤1 connection - possible missing edges or undocumented components.
- **2 thin communities (<3 nodes) omitted from report** — run `graphify query` to explore isolated nodes.

## Suggested Questions
_Questions this graph is uniquely positioned to answer:_

- **Why does `DbPackage` connect `Local Turso DB Layer` to `Pubgrub Constraint Parsing`, `Client Install Action Logic`, `Client DB Test Suite`, `Core Package Data Model`?**
  _High betweenness centrality (0.125) - this node is a cross-community bridge._
- **Why does `Dependency` connect `Core Package Data Model` to `Pubgrub Constraint Parsing`, `Local Turso DB Layer`?**
  _High betweenness centrality (0.105) - this node is a cross-community bridge._
- **Why does `Db` connect `Local Turso DB Layer` to `Client Install Action Logic`, `Client DB Test Suite`, `Zip File Utilities`?**
  _High betweenness centrality (0.069) - this node is a cross-community bridge._
- **What connects `package_name`, `file_name`, `version` to the rest of the system?**
  _37 weakly-connected nodes found - possible documentation gaps or missing edges._
- **Should `Project Docs & Design Rationale` be split into smaller, more focused modules?**
  _Cohesion score 0.06240084611316764 - nodes in this community are weakly interconnected._
- **Should `Pubgrub Constraint Parsing` be split into smaller, more focused modules?**
  _Cohesion score 0.07878787878787878 - nodes in this community are weakly interconnected._
- **Should `Client Install Action Logic` be split into smaller, more focused modules?**
  _Cohesion score 0.11149825783972125 - nodes in this community are weakly interconnected._