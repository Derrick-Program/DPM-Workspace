# DPM-Workspace

> **範圍規則(嚴格遵守)**:本檔案只放 DPM-Workspace 這個專案專屬的資訊。跨專案的個人通用偏好(全域 Git Workflow 分支模型、gstack 工具使用規則等)已收斂於 `~/.claude/CLAUDE.md`,不在此重複;若本專案需要對通用預設值做例外,才在此明確註記覆寫,並說明原因。

DPM (Derrick Package Manager) 的 Cargo workspace,整合原本三個獨立 repo。

## 結構

```
crates/
├── dpm/         # Client CLI (bin: dpm) — 安裝/更新/移除套件
├── dpm-server/  # Server CLI (bin: dpm-server) — 管理 Repo 與 RepoInfo.json
└── dpm-core/    # 共用 lib (dpm_core) — RepoInfo, PackageInfo, JsonStorage, error types
```

- 共用 dependency 版本統一放在根 `Cargo.toml` 的 `[workspace.dependencies]`,crate 內用 `xxx.workspace = true`。
- `DPM-Core` 以 path 依賴,不再走 git dependency。

## 架構重點

- **Features**:`dpm-core` 有 `client` / `server` 兩個 feature,只 gate `impl` 區塊,**不可以 gate struct 欄位**(workspace 的 feature unification 會讓單獨編譯與整體編譯行為不同,之前就是這樣炸的)。feature 必須保持 additive。`RepoInfo` 的 CRUD 方法(`add_package`/`update_package`/`remove_package`)在 `server` feature 下,`fetch_update_repo_info`/`fetch_package`/`get_single_package_info` 在 `client` feature 下。
- **Client 資料層**:diesel + SQLite,DB 在 `/opt/DPM/LocalRepo.db`,migration 用 `embed_migrations!` 內嵌(`crates/dpm/migrations/`),schema 在 `src/utils/schema.rs`。用 fs2 file lock(`/opt/DPM/LocalRepo.lock`)防多實例。`crates/dpm/diesel.toml` 的 `migrations_directory` 是舊 repo 分割前留下的絕對路徑,已經指向不存在的位置 —— 執行 `just migration-new`/`migration-run` 前先確認 diesel CLI 實際寫入/讀取到哪裡,別假設它對著 `crates/dpm/migrations`。
- **權限模型**:Linux 需 root(`sudo::escalate_if_needed` 自動提權),macOS 用 `sudo` 呼叫個別指令。`SUDO_USER` 用來在提權後拿真實使用者做 chown。初始化順序:先確保 `/opt/DPM` 存在 → 開 DB → 其他動作(見 `dpm/src/lib.rs::set_globle_var` 與 `main.rs`)。
- **Client CLI 手刻 vs Server CLI derive**:`dpm` 的 `cli_parse.rs` 用 `clap::Command` 手動建構(`build_cli()`)並在 `get_args()` 手動 match 每個 subcommand 填 `Cli` struct;`dpm-server` 用 `#[derive(Parser)]`/`#[derive(Subcommand)]`。兩邊新增 subcommand 的改法不同,改 `dpm` 那邊要同時改 `build_cli()` 與 `get_args()` 的 match。
- **Server 資料**:`Repo/` 放打包好的 `.zip`,`Repo/src/<pkg>/` 放原始碼 + `packageInfo.json` + `hashes.json`,索引是 `RepoInfo.json`。`dpm-server` 的四個子指令對應 `action.rs`:`init`(建立套件骨架)→`hash`(算 `Repo/src/<pkg>/` 下所有檔案的 SHA256 寫入 `hashes.json`,並回填 `packageInfo.json.hash`)→`build`(zip 打包到 `Repo/<pkg>.zip`)→`fix add/del`(把套件加入/移除 `RepoInfo.json` 索引)。
- **序列化相容**:`PackageBasicInfo` 的 `entry`/`description` 是 `Option` + `#[serde(default, skip_serializing_if)]`,改欄位時注意舊 JSON 相容性。
- **重複實作**:`zip_folder`/`unzip_file`/`read_file_from_zip`(`dpm/src/utils/zip_file.rs` vs `dpm-server/src/zip_file.rs`)與 SHA256 `hasher()`(`dpm/src/action.rs` vs `dpm-server/src/action.rs`)各自複製一份,沒有共用到 `dpm-core`,兩邊 `unzip_file` 簽名也不同(client 版少一個 `name` 參數)。改其中一份時記得檢查另一份是否也要同步改。
- `crates/dpm-server/src/json_parse1.rs` 是分割前留下的死碼(`PackageInfo`/`RepoInfo`/`JsonStorage` 的舊版複本),`main.rs` 沒有把它列進 `mod`,不會被編譯進二進位檔。真正用的是 `dpm_core` 的同名型別,不要誤改到這份。

## 已知待處理問題

- `crates/dpm/src/utils/system.rs` 的 `init()`:`repo_url`/`repo_info` 塞進 config HashMap 後沒有寫回 `config.json`(檔案永遠是 `{}`)。
- 權限模型不一致:Linux `chown -R root:root`,macOS `chown user:admin`。
- `PackageManager::Unknown` 與 unsupported OS 走 `panic!`,應改為錯誤回傳(`system.rs` 裡多處 `match` 分支)。

## 常用指令

一律用 `just`(見 `justfile`):`just check`、`just test`、`just lint`、`just run-client <args>`、`just run-server <args>`。

## Superpowers spec-driven workflow

本專案用 **Superpowers** skill 套件做變更管理(OpenSpec 評估過後放棄,不要重新引入 `openspec/`)。流程:`brainstorming` → `writing-plans` → `executing-plans` / `subagent-driven-development`。

- Design doc:`docs/superpowers/specs/YYYY-MM-DD-<topic>-design.md`
- Implementation plan:`docs/superpowers/plans/YYYY-MM-DD-<change-name>.md`

## Secrets (Infisical)

環境設定集中放在 **Infisical**(environment slug:`dev` / `staging` / `prod`;若其他工具鏈用 `ENV=test` 這個名稱,對應到 `staging` slug)。`justfile` 裡所有 recipe 都透過 `infisical run --env=<env> --path=/ --command=...` 執行,secret 只在執行期注入,不落地到硬碟。預設 environment 是 `dev`,用 `DPM_ENV=staging just <recipe>` 覆寫。

本機開發用互動 OAuth 登入(`just env-login`,每台機器一次)+ repo 連結檔 `.infisical.json`(`just env-init` 產生,一個 repo 一次,內容只有 project ID,可進版控)。CI/CD 用 Machine Identity token,經 `INFISICAL_TOKEN` 環境變數注入,不走互動登入。

管理 secret:`just env-list` 列出目前 environment 的 key(不印值)、`just env-push <dotenv-file> <env>` 批次匯入既有 dotenv 檔案,或直接用 Infisical dashboard。

`crates/dpm/.env` 已退役(`DATABASE_URL` 改由 Infisical `dev` environment 提供),不要重新加回來;SOPS 與 `envs/*.json` 這類舊方案也不要重新引入。`.env` 檔案一律不進版控。

## 慣例

- Edition 2021。package 名稱維持 `DPM` / `DPM-Server` / `DPM-Core`(歷史因素,勿改名以免破壞下游)。
- 錯誤處理:`thiserror` 定義 error enum(`CoreError` / `ClientError`),binary 進入點可用 `anyhow`。
- 提交前至少跑 `just pre-commit`(fmt + clippy + test)。
