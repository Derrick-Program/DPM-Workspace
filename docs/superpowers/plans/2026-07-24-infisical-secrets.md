# Infisical Secrets Integration Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.
>
> **Task 1 is a manual/human-only prerequisite.** It requires interactive browser OAuth against the user's real Infisical org and cannot be performed by an agent (no credentials, no browser). The user must complete Task 1 themselves before any subagent starts Task 2. Tasks 2–5 are normal agent-executable tasks, but their verification steps assume Task 1 is already done.

**Goal:** Move DPM-Workspace's environment/secret handling into Infisical so every `just` recipe gets secrets injected at runtime instead of relying on a committed-or-not `.env` file.

**Architecture:** A single `env` variable in the root `justfile` (sourced from `DPM_ENV`, default `dev`) is interpolated into `infisical run --env={{env}} --path=/ --command="..."` calls that wrap every existing recipe body. Four new recipes (`env-login`, `env-init`, `env-list`, `env-push`) manage the Infisical connection itself. `crates/dpm/.env` is retired; its one value (`DATABASE_URL`) moves into the Infisical `dev` environment.

**Tech Stack:** `just` (task runner, already in use), Infisical CLI (`infisical`), existing Rust/Cargo/diesel toolchain (unchanged).

## Global Constraints

- Infisical environment slugs: `dev` / `staging` / `prod`. If the user's other tooling refers to `ENV=test`, that maps to the `staging` slug — this is a naming convention only, not a 4th environment.
- Every recipe in `justfile` must route secrets through Infisical (per approved design, no exceptions carved out for "recipes that don't need secrets today").
- No SOPS, no `envs/*.json`, no reintroducing committed `.env` files. `.env` stays git-ignored and is not used by any recipe going forward.
- Do not touch the unused `dotenv = "0.15.0"` dependency in `crates/dpm/Cargo.toml` — confirmed unused (`grep dotenv` over `crates/**/*.rs` returns nothing), but removing it is out of scope for this change (spec: `docs/superpowers/specs/2026-07-24-infisical-secrets-design.md`, "Secrets 遷移" section).
- Real, verified CLI syntax only (checked against https://infisical.com/docs/cli/commands/{run,init,login,secrets} on 2026-07-24) — do not invent flags.

---

### Task 1: Infisical project setup and local auth (MANUAL — not agent-executable)

**Files:** none (no repo files change in this task; `.infisical.json` is created but treated as an output the later tasks/verifications depend on)

**Interfaces:**
- Produces: a working `.infisical.json` in the repo root (created by `infisical init`), and an authenticated local Infisical CLI session, that Tasks 2–5's verification steps depend on.

- [ ] **Step 1: Install the Infisical CLI if not already present**

Run: `infisical --version`
Expected: prints a version string. If the command is not found, install it first (e.g. `brew install infisical/get-cli/infisical` on macOS) before continuing.

- [ ] **Step 2: Log in interactively**

Run: `infisical login`
Expected: opens a browser OAuth flow; on success the CLI prints a confirmation and stores a local session.

- [ ] **Step 3: Create (or select) the `DPM-Workspace` project in your existing Infisical org**

Do this in the Infisical dashboard if `infisical init` doesn't offer project creation inline. New Infisical projects scaffold `dev` / `staging` / `prod` environments by default — confirm those three exist; if not, add them in the dashboard.

- [ ] **Step 4: Link this repo to the project**

Run (from repo root): `infisical init`
Expected: interactive prompt to pick org → `DPM-Workspace` project; creates `.infisical.json` in the repo root containing the project ID (not a secret — Infisical's own docs recommend committing this file).

- [ ] **Step 5: Verify the connection**

Run: `infisical secrets --env=dev --path=/`
Expected: exits 0 and prints an (initially empty) secrets table for the `dev` environment. A non-zero exit or an auth error means Steps 2 or 4 didn't complete — redo them before moving to Task 2.

---

### Task 2: Wrap all justfile recipes with Infisical secret injection

**Files:**
- Modify: `justfile` (full rewrite of recipe bodies; recipe names and existing call signatures — `test-p crate`, `run-client *args`, `run-server *args`, `migration-new name`, `env-push dotenv target_env` — are preserved so existing invocations like `just test-p DPM` keep working)

**Interfaces:**
- Produces: `env-login`, `env-init`, `env-list`, `env-push <dotenv> <target_env>` recipes that Task 3's steps and the CLAUDE.md doc in Task 4 reference by exact name.
- Consumes: Task 1's authenticated `infisical` CLI session and `.infisical.json` (only needed to *run* the recipes, not to write this task's diff).

**Design note (deviation from the original chat proposal):** the spec/chat sketch suggested overriding environment with `just check env=staging` (a recipe parameter). That collides with `just`'s positional-argument binding on recipes that already take required params (`test-p crate`, `migration-new name`) or variadic args (`run-client *args`) — `just run-client install foo` would bind `install` to `env` instead of to `args`, silently breaking existing invocations. Using a justfile-level variable sourced from the `DPM_ENV` environment variable (`DPM_ENV=staging just check`) avoids the collision entirely and needs no per-recipe parameter. This task implements the variable-based approach, not the parameter-based one.

- [ ] **Step 1: Replace the entire contents of `justfile`**

```just
# DPM-Workspace 常用指令
# 用法: just <recipe>,列出全部: just --list
# Secret 一律透過 Infisical 注入,執行前需先 `just env-login` + `just env-init`(見下方 Secrets 區塊)。
# 預設用 dev environment,可用 DPM_ENV=staging just <recipe> 覆寫。

env := env_var_or_default("DPM_ENV", "dev")

# 預設顯示可用指令
default:
    @just --list

# ── 開發 ────────────────────────────────────────────

# 快速檢查整個 workspace
check:
    infisical run --env={{env}} --path=/ --command="cargo check --workspace"

# 編譯 (debug)
build:
    infisical run --env={{env}} --path=/ --command="cargo build --workspace"

# 編譯 (release, 已開 lto + strip)
release:
    infisical run --env={{env}} --path=/ --command="cargo build --workspace --release"

# 跑全部測試
test:
    infisical run --env={{env}} --path=/ --command="cargo test --workspace"

# 跑指定 crate 的測試, 例: just test-p DPM
test-p crate:
    infisical run --env={{env}} --path=/ --command="cargo test -p {{crate}}"

# clippy (warning 視為錯誤)
lint:
    infisical run --env={{env}} --path=/ --command="cargo clippy --workspace --all-targets -- -D warnings"

# clippy 自動修
lint-fix:
    infisical run --env={{env}} --path=/ --command="cargo clippy --workspace --all-targets --fix --allow-dirty"

# 格式化
fmt:
    infisical run --env={{env}} --path=/ --command="cargo fmt --all"

# 檢查格式 (CI 用)
fmt-check:
    infisical run --env={{env}} --path=/ --command="cargo fmt --all -- --check"

# 提交前檢查: 格式 + clippy + 測試
pre-commit: fmt lint test

# ── 執行 ────────────────────────────────────────────

# 跑 client, 例: just run-client install foo
run-client *args:
    infisical run --env={{env}} --path=/ --command="cargo run -p DPM -- {{args}}"

# 跑 server, 例: just run-server init
run-server *args:
    infisical run --env={{env}} --path=/ --command="cargo run -p DPM-Server -- {{args}}"

# ── Diesel (client DB) ─────────────────────────────

# 新增 migration, 例: just migration-new add_column
migration-new name:
    infisical run --env={{env}} --path=/ --command="cd crates/dpm && diesel migration generate {{name}}"

# 套用 migrations (DATABASE_URL 由 Infisical 注入)
migration-run:
    infisical run --env={{env}} --path=/ --command="cd crates/dpm && diesel migration run"

# 重跑最後一個 migration
migration-redo:
    infisical run --env={{env}} --path=/ --command="cd crates/dpm && diesel migration redo"

# ── 文件與維護 ──────────────────────────────────────

# 產生並開啟文件
doc:
    infisical run --env={{env}} --path=/ --command="cargo doc --workspace --no-deps --open"

# 清除編譯產物
clean:
    infisical run --env={{env}} --path=/ --command="cargo clean"

# 檢查過期 dependency (需 cargo-outdated)
outdated:
    infisical run --env={{env}} --path=/ --command="cargo outdated --workspace"

# 檢查安全性漏洞 (需 cargo-audit)
audit:
    infisical run --env={{env}} --path=/ --command="cargo audit"

# 更新 Cargo.lock
update:
    infisical run --env={{env}} --path=/ --command="cargo update"

# ── 安裝 ────────────────────────────────────────────

# 安裝 dpm client 到 ~/.cargo/bin
install-client:
    infisical run --env={{env}} --path=/ --command="cargo install --path crates/dpm"

# 安裝 dpm-server 到 ~/.cargo/bin
install-server:
    infisical run --env={{env}} --path=/ --command="cargo install --path crates/dpm-server"

# ── Secrets (Infisical) ─────────────────────────────

# 互動登入 Infisical(每台機器一次)
env-login:
    infisical login

# 建立/連結 .infisical.json 到既有 Infisical project(每個 repo 一次)
env-init:
    infisical init

# 列出目前 environment 的 secret(不印值)
env-list:
    infisical secrets --env={{env}} --path=/

# 批次匯入既有 dotenv 檔案內容到指定 environment, 例: just env-push crates/dpm/.env dev
env-push dotenv target_env:
    infisical secrets set --file="{{dotenv}}" --env={{target_env}}
```

Note: `pre-commit: fmt lint test` and `default: @just --list` are left as plain dependency/introspection recipes with no body of their own — they're covered transitively because `fmt`/`lint`/`test` are each already wrapped, and `default` never touches cargo or secrets.

- [ ] **Step 2: Sanity-check the justfile parses**

Run: `just --list`
Expected: prints the full recipe list (same names as before, plus `env-login`, `env-init`, `env-list`, `env-push`) with no parse errors.

- [ ] **Step 3: Confirm existing call patterns still bind correctly**

Run: `just --dry-run test-p DPM` and `just --dry-run run-client install foo`
Expected: the dry-run output shows `cargo test -p DPM` and `cargo run -p DPM -- install foo` inside the printed `infisical run --command="..."` line — i.e. `DPM` and `install foo` land in `{{crate}}` / `{{args}}`, not in `{{env}}`.

- [ ] **Step 4: Commit**

```bash
git add justfile
git commit -m "feat(justfile): route all recipes through Infisical secret injection"
```

---

### Task 3: Migrate `DATABASE_URL` into Infisical and retire `crates/dpm/.env`

**Files:**
- Delete: `crates/dpm/.env`

**Interfaces:**
- Consumes: `env-push` recipe from Task 2 (`just env-push <dotenv> <env>`), Task 1's authenticated session.

- [ ] **Step 1: Push the existing value into the `dev` environment**

Run: `just env-push crates/dpm/.env dev`
Expected: exits 0; `infisical secrets set --file="crates/dpm/.env" --env=dev` reads the single `DATABASE_URL=./LocalRepo.db` line and creates that key in Infisical.

- [ ] **Step 2: Verify it landed**

Run: `just env-list`
Expected: output table includes a row for `DATABASE_URL`.

- [ ] **Step 3: Delete the local dotenv file**

```bash
git rm crates/dpm/.env
```

(If it was never tracked by git — check first with `git ls-files crates/dpm/.env`; if that prints nothing, it's already git-ignored/untracked, so use `rm crates/dpm/.env` instead of `git rm`.)

- [ ] **Step 4: Verify diesel still resolves `DATABASE_URL` through Infisical**

Run: `just migration-run`
Expected: diesel connects successfully (either applies pending migrations or reports none pending — it must NOT error with "DATABASE_URL must be set" or a connection error).

- [ ] **Step 5: Commit**

```bash
git add -A crates/dpm/.env
git commit -m "chore: retire crates/dpm/.env, DATABASE_URL now comes from Infisical dev env"
```

(If Step 3 used `rm` instead of `git rm` because the file was untracked, skip `git add` for it — there's nothing to stage — and just confirm `git status` shows no leftover reference to the file.)

---

### Task 4: Document Infisical usage in CLAUDE.md

**Files:**
- Modify: `CLAUDE.md` (insert a new `## Secrets (Infisical)` section; place it after `## 常用指令` and before `## 慣例`, since it's operational/how-to content like the commands section above it)

**Interfaces:**
- Consumes: exact recipe names from Task 2 (`env-login`, `env-init`, `env-list`, `env-push`) and the `DPM_ENV` variable name — must match verbatim.

- [ ] **Step 1: Insert the section**

In `CLAUDE.md`, right before the `## 慣例` heading, add:

```markdown
## Secrets (Infisical)

環境設定集中放在 **Infisical**(environment slug:`dev` / `staging` / `prod`;若其他工具鏈用 `ENV=test` 這個名稱,對應到 `staging` slug)。`justfile` 裡所有 recipe 都透過 `infisical run --env=<env> --path=/ --command=...` 執行,secret 只在執行期注入,不落地到硬碟。預設 environment 是 `dev`,用 `DPM_ENV=staging just <recipe>` 覆寫。

本機開發用互動 OAuth 登入(`just env-login`,每台機器一次)+ repo 連結檔 `.infisical.json`(`just env-init` 產生,一個 repo 一次,內容只有 project ID,可進版控)。CI/CD 用 Machine Identity token,經 `INFISICAL_TOKEN` 環境變數注入,不走互動登入。

管理 secret:`just env-list` 列出目前 environment 的 key(不印值)、`just env-push <dotenv-file> <env>` 批次匯入既有 dotenv 檔案,或直接用 Infisical dashboard。

`crates/dpm/.env` 已退役(`DATABASE_URL` 改由 Infisical `dev` environment 提供),不要重新加回來;SOPS 與 `envs/*.json` 這類舊方案也不要重新引入。`.env` 檔案一律不進版控。
```

- [ ] **Step 2: Verify placement and rendering**

Run: `grep -n "^## " CLAUDE.md`
Expected: headings appear in this order: `結構`, `架構重點`, `已知待處理問題`, `常用指令`, `Superpowers spec-driven workflow`, `Secrets (Infisical)`, `慣例` (the new section sits between the existing `常用指令`/`Superpowers spec-driven workflow` block and `慣例`).

- [ ] **Step 3: Commit**

```bash
git add CLAUDE.md
git commit -m "docs: document Infisical secrets workflow in CLAUDE.md"
```

---

### Task 5: End-to-end verification

**Files:** none (verification only)

**Interfaces:** none — this task only runs commands produced by Tasks 2–4.

- [ ] **Step 1: Full recipe smoke test**

Run: `just check`
Expected: normal `cargo check --workspace` output, exit 0. Confirms the Infisical wrapper doesn't break a plain recipe.

- [ ] **Step 2: Secret-dependent recipe smoke test**

Run: `just migration-run`
Expected: exit 0, diesel reports either applied migrations or nothing pending — no `DATABASE_URL` error.

- [ ] **Step 3: Confirm working tree is clean**

Run: `git status --short`
Expected: empty output (everything from Tasks 2–4 already committed) except possibly untracked build artifacts already covered by `.gitignore` (`/target`).

- [ ] **Step 4: Confirm no stray secret files**

Run: `git ls-files | grep -E "\.env$"`
Expected: empty output — no `.env` file tracked anywhere in the repo.
