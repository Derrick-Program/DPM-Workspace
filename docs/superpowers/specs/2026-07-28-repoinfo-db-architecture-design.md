# RepoInfo.db & Dual-Database Architecture Design

**Date**: 2026-07-28
**Status**: Approved
**Scope**: `crates/dpm-core`, `crates/dpm`, `crates/dpm-server`

---

## 1. Context & Motivation

DPM currently uses a single JSON index file (`RepoInfo.json`) for remote package index serving, and a single client database (`LocalRepo.db`) for both available remote packages and installed local package records.

As package registries scale, single JSON index files suffer from performance degradation due to full-file JSON parsing overhead. Furthermore, mixing remote index cache and local installation state in a single DB table makes state cleanup, backup, and isolation difficult.

This design introduces a **Pure SQLite Dual-Database Architecture**:

1. **Server Index**: Replaces `RepoInfo.json` with `RepoInfo.db` (a static SQLite database committed and served directly).
2. **Client Cache Isolation**: Separates client state into `LocalRepoInfo.db` (remote index cache) and `LocalRepo.db` (local installed packages & file manifest).

---

## 2. System Architecture

```
[Server: dpm-server]
  │
  ├── packages/hello/ ...
  └── crates/dpm-server/RepoInfo.db  (SQLite DB committed to git & served via HTTPS)
        │
        ▼ (HTTPS / Raw download via `dpm update`)
[Client: dpm]
  ├── ~/.local/share/dpm/LocalRepoInfo.db  (Remote index cache for all subscribed sources)
  └── ~/.local/share/dpm/LocalRepo.db      (Local installed packages & installed_files manifest)
```

---

## 3. Database Schemas

### 3.1 `LocalRepoInfo.db` (Remote Index Cache)

Stores available packages from all configured sources (`official`, third-party sources).

```sql
CREATE TABLE IF NOT EXISTS AvailablePackages (
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
    targets TEXT,
    PRIMARY KEY (source, name, version)
);
```

### 3.2 `LocalRepo.db` (Local Installed Status & File Manifest)

Stores packages currently installed on the local system and their created symlinks/files.

```sql
CREATE TABLE IF NOT EXISTS InstalledPackages (
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
    installed_at DATETIME DEFAULT CURRENT_TIMESTAMP,
    PRIMARY KEY (name)
);

CREATE TABLE IF NOT EXISTS installed_files (
    package_name TEXT NOT NULL,
    file_path TEXT NOT NULL,
    PRIMARY KEY (package_name, file_path)
);
```

### 3.3 `dpm-server` `RepoInfo.db`

The server database schema matches `AvailablePackages` without the `source` column (since source alias is assigned by client configuration):

```sql
CREATE TABLE IF NOT EXISTS Packages (
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
    targets TEXT,
    PRIMARY KEY (name, version)
);
```

---

## 4. Workflows & Interfaces

### 4.1 Server (`dpm-server`) Workflows

- **`dpm-server init/build/hash/sign/fix add`**:
  Instead of reading/writing `RepoInfo.json`, all subcommands open and execute SQL queries on `crates/dpm-server/RepoInfo.db`.
- **Default Repo File**: `crates/dpm-server/RepoInfo.db` is committed to git tracking. `RepoInfo.json` is deprecated and removed.

### 4.2 Client (`dpm`) Workflows

- **`dpm update`**:
  1. Downloads `RepoInfo.db` from each source (e.g. `https://raw.githubusercontent.com/Derrick-Program/DPM-Workspace/main/crates/dpm-server/RepoInfo.db` for `official`).
  2. Clears and populates the source's entries in `LocalRepoInfo.db`.
- **`dpm search <query>`**:
  Performs case-insensitive substring search against `LocalRepoInfo.db`.
- **`dpm install <pkg>`**:
  1. Queries `LocalRepoInfo.db` for the requested package and target-compatible version.
  2. If the package does not exist in `LocalRepoInfo.db`, prints an explicit user prompt:
     `Error: Package '<pkg>' not found in local cache. Please run 'dpm update' to refresh package index.`
  3. Upon successful download/build and installation, writes the installed record and linked file manifest to `LocalRepo.db`.
- **`dpm uninstall <pkg>`**:
  Queries `LocalRepo.db` for installed files manifest (`installed_files`), deletes symlinks in $O(1)$ time, and removes records from `LocalRepo.db`.

---

## 5. Acceptance Criteria

1. `crates/dpm-server/RepoInfo.json` is replaced by `crates/dpm-server/RepoInfo.db`.
2. All `dpm-server` commands operate cleanly on `RepoInfo.db`.
3. Client maintains two separate databases: `LocalRepoInfo.db` and `LocalRepo.db`.
4. `dpm update` updates `LocalRepoInfo.db` without touch to `LocalRepo.db`.
5. `dpm install` queries `LocalRepoInfo.db` and outputs a clear prompt if package is missing.
6. All existing unit & integration tests pass cleanly across the workspace.
