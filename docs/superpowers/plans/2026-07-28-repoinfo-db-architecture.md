# RepoInfo.db & Dual-Database Architecture Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Transition DPM from legacy `RepoInfo.json` to SQLite `RepoInfo.db` on server, and split client cache into `LocalRepoInfo.db` (remote index cache) and `LocalRepo.db` (installed packages & file manifest).

**Architecture:** Server `dpm-server` builds and updates `RepoInfo.db` directly. Client `dpm update` downloads remote `RepoInfo.db` files into `LocalRepoInfo.db`. Client `dpm install` checks `LocalRepoInfo.db`, installs packages, and writes installed status & file manifest into `LocalRepo.db`.

**Tech Stack:** Rust, SQLite (`turso`), `include_dir`, Cargo Workspace.

## Global Constraints

- Server repo index: `crates/dpm-server/RepoInfo.db`
- Client remote cache DB: `LocalRepoInfo.db`
- Client local installed status DB: `LocalRepo.db`
- Official repo info URL: `https://raw.githubusercontent.com/Derrick-Program/DPM-Workspace/main/crates/dpm-server/RepoInfo.db`

---

### Task 1: Server SQLite Index Database (`dpm-server`)

**Files:**
- Modify: `crates/dpm-server/src/action.rs`
- Modify: `crates/dpm-server/src/config.rs`
- Modify: `crates/dpm-server/src/cli_parse.rs`
- Create: `crates/dpm-server/RepoInfo.db`
- Delete: `crates/dpm-server/RepoInfo.json`

**Interfaces:**
- Consumes: Package metadata struct `RepoInfo` / `RepoPackageInfo`
- Produces: `crates/dpm-server/RepoInfo.db` SQLite database file

- [ ] **Step 1: Write tests for dpm-server RepoInfo.db operations**

Write test verifying creating and querying `RepoInfo.db` SQLite file.

- [ ] **Step 2: Update dpm-server action methods to write RepoInfo.db**

Replace serde JSON read/write in `dpm-server` with turso/SQLite table operations on `Packages`.

- [ ] **Step 3: Create initial RepoInfo.db and remove RepoInfo.json**

Generate `crates/dpm-server/RepoInfo.db` containing initial `hello` and `addsub` package versions, and delete `RepoInfo.json`.

- [ ] **Step 4: Verify dpm-server tests pass**

Run: `cargo test -p DPM-Server`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add crates/dpm-server/
git commit -m "feat: replace RepoInfo.json with SQLite RepoInfo.db in dpm-server"
```

---

### Task 2: Client Dual Database Infrastructure (`LocalRepoInfo.db` & `LocalRepo.db`)

**Files:**
- Modify: `crates/dpm/src/context.rs`
- Modify: `crates/dpm/src/utils/db.rs`
- Modify: `crates/dpm/src/utils/system.rs`

**Interfaces:**
- Consumes: `Context` paths
- Produces: `Context::available_db(&self)` for `LocalRepoInfo.db` and `Context::db(&self)` for `LocalRepo.db`

- [ ] **Step 1: Update Context to support both LocalRepoInfo.db and LocalRepo.db**

Add `local_repo_info_db_path` to `Context` and update directory initializers.

- [ ] **Step 2: Update Db in db.rs to handle AvailablePackages vs InstalledPackages**

Implement `AvailableDb` or helper methods on `Db` to read/write `LocalRepoInfo.db` (`AvailablePackages`) and `LocalRepo.db` (`InstalledPackages` & `installed_files`).

- [ ] **Step 3: Run db unit tests**

Run: `cargo test -p DPM db_tests`
Expected: PASS

- [ ] **Step 4: Commit**

```bash
git add crates/dpm/src/context.rs crates/dpm/src/utils/db.rs crates/dpm/src/utils/system.rs
git commit -m "feat: implement dual-database isolation (LocalRepoInfo.db and LocalRepo.db) in client"
```

---

### Task 3: Client Sync (`dpm update`) and Substring Search (`dpm search`)

**Files:**
- Modify: `crates/dpm/src/action.rs`
- Modify: `crates/dpm/src/utils/fetcher.rs`

**Interfaces:**
- Consumes: `LocalRepoInfo.db`
- Produces: `dpm update` & `dpm search` actions

- [ ] **Step 1: Update fetcher to download RepoInfo.db SQLite file**

Update `fetcher.rs` to fetch remote `.db` file directly via HTTPS or file copy.

- [ ] **Step 2: Update dpm update to import remote RepoInfo.db into LocalRepoInfo.db**

Update `sync_source_inner` to read remote `RepoInfo.db` entries and update `LocalRepoInfo.db`.

- [ ] **Step 3: Update dpm search to query LocalRepoInfo.db**

Update `search` to execute substring search on `LocalRepoInfo.db`.

- [ ] **Step 4: Verify sync & search tests**

Run: `cargo test -p DPM search`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add crates/dpm/src/action.rs crates/dpm/src/utils/fetcher.rs
git commit -m "feat: update dpm update and dpm search to use LocalRepoInfo.db"
```

---

### Task 4: Client Install & Uninstall (`dpm install` & `dpm uninstall`)

**Files:**
- Modify: `crates/dpm/src/action.rs`

**Interfaces:**
- Consumes: `LocalRepoInfo.db` (for lookup) & `LocalRepo.db` (for install state)
- Produces: `dpm install` & `dpm uninstall`

- [ ] **Step 1: Update dpm install to lookup in LocalRepoInfo.db and suggest dpm update on missing**

Update `install` to query `LocalRepoInfo.db`. If package not found, print explicit prompt:
`Error: Package '<pkg>' not found in local cache. Please run 'dpm update' to refresh package index.`

- [ ] **Step 2: Write installed record and manifest to LocalRepo.db**

Upon successful installation, write to `LocalRepo.db` (`InstalledPackages` & `installed_files`).

- [ ] **Step 3: Update uninstall to read LocalRepo.db manifest**

Update `uninstall` to query `installed_files` from `LocalRepo.db` and delete symlinks in $O(1)$ time.

- [ ] **Step 4: Verify end-to-end install & uninstall tests**

Run: `cargo test --workspace`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add crates/dpm/src/action.rs
git commit -m "feat: update dpm install and uninstall to use dual-database model"
```

---

### Task 5: End-to-End Verification & Documentation Update

**Files:**
- Modify: Obsidian report in `/Users/derrick/Library/Mobile Documents/com~apple~CloudDocs/AI-資料庫/代碼審查記錄/`

- [ ] **Step 1: Run full workspace tests and clippy**

Run: `cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace`
Expected: PASS

- [ ] **Step 2: Update Obsidian audit report**

Record completion of `RepoInfo.db` and dual-database architecture.

- [ ] **Step 3: Commit**

```bash
git add -A
git commit -m "chore: complete RepoInfo.db transition and update documentation"
```
