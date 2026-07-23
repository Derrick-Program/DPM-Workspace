# DPM-Workspace

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

- **Features**:`dpm-core` 有 `client` / `server` 兩個 feature,只 gate `impl` 區塊,**不可以 gate struct 欄位**(workspace 的 feature unification 會讓單獨編譯與整體編譯行為不同,之前就是這樣炸的)。feature 必須保持 additive。
- **Client 資料層**:diesel + SQLite,DB 在 `/opt/DPM/LocalRepo.db`,migration 用 `embed_migrations!` 內嵌(`crates/dpm/migrations/`),schema 在 `src/utils/schema.rs`。用 fs2 file lock(`/opt/DPM/LocalRepo.lock`)防多實例。
- **權限模型**:Linux 需 root(`sudo::escalate_if_needed` 自動提權),macOS 用 `sudo` 呼叫個別指令。`SUDO_USER` 用來在提權後拿真實使用者做 chown。初始化順序:先確保 `/opt/DPM` 存在 → 開 DB → 其他動作。
- **Server 資料**:`Repo/` 放打包好的 `.zip`,`Repo/src/<pkg>/` 放原始碼 + `packageInfo.json` + `hashes.json`,索引是 `RepoInfo.json`。
- **序列化相容**:`PackageBasicInfo` 的 `entry`/`description` 是 `Option` + `#[serde(default, skip_serializing_if)]`,改欄位時注意舊 JSON 相容性。

## 已知待處理問題

- `crates/dpm/src/utils/system.rs` 的 `init()`:`repo_url`/`repo_info` 塞進 config HashMap 後沒有寫回 `config.json`(檔案永遠是 `{}`)。
- 權限模型不一致:Linux `chown -R root:root`,macOS `chown user:admin`。
- `PackageManager::Unknown` 與 unsupported OS 走 `panic!`,應改為錯誤回傳。

## 常用指令

一律用 `just`(見 `justfile`):`just check`、`just test`、`just lint`、`just run-client <args>`、`just run-server <args>`。

## 慣例

- Edition 2021。package 名稱維持 `DPM` / `DPM-Server` / `DPM-Core`(歷史因素,勿改名以免破壞下游)。
- 錯誤處理:`thiserror` 定義 error enum(`CoreError` / `ClientError`),binary 進入點可用 `anyhow`。
- 提交前至少跑 `just pre-commit`(fmt + clippy + test)。
