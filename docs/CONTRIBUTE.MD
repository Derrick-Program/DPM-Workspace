# Contributing to DPM-Workspace

給要動這個 repo 原始碼的人看。安裝、日常使用 `dpm`/`dpm-server` 的說明在 [`README.md`](../README.md);架構細節、已知問題見 [`CLAUDE.md`](../CLAUDE.md)。

```
crates/
├── dpm/         # Client CLI (bin: dpm)
├── dpm-server/  # Server CLI (bin: dpm-server)
└── dpm-core/    # 共用 lib
```

## 前置設定

指令一律透過 `just` 執行,secret 由 [Infisical](https://infisical.com) 在執行期注入,不落地到硬碟。

```bash
just env-login   # 互動 OAuth 登入,每台機器一次
just env-init    # 產生/連結 .infisical.json,每個 repo 一次
```

沒設定過 Infisical project 的話,`env-init` 會需要有人先在 Infisical dashboard 建好 project 並把 `dev`/`staging`/`prod` 三個 environment 填好基本 secret(至少 client 端不需要額外 secret 也能跑,`DATABASE_URL` 已經退役)。

預設用 `dev` environment,要切換用:

```bash
DPM_ENV=staging just <recipe>
```

## justfile 指令總覽

跑 `just --list` 可以隨時列出全部指令。以下依用途分類。

### 開發

| 指令                    | 說明                                                         |
| ----------------------- | ------------------------------------------------------------ |
| `just check`          | `cargo check --workspace`,快速檢查編譯                     |
| `just build`          | `cargo build --workspace`(debug)                           |
| `just release`        | `cargo build --workspace --release`(開 lto + strip)        |
| `just test`           | `cargo test --workspace`                                   |
| `just test-p <crate>` | 只測單一 crate,例:`just test-p DPM`                        |
| `just lint`           | `cargo clippy --workspace --all-targets -- -D warnings`    |
| `just lint-fix`       | clippy 自動修                                                |
| `just fmt`            | `cargo fmt --all`                                          |
| `just fmt-check`      | 檢查格式,不修改(CI 用)                                       |
| `just pre-commit`     | `fmt` + `lint` + `test` 一次跑完,commit 前建議先跑這個 |

### 執行(Client / Server)

```bash
just run-client <args>   # cargo run -p DPM -- <args>
just run-server <args>   # cargo run -p DPM-Server -- <args>
```

`<args>` 是直接轉給該 binary 的參數,子指令、flag 都照原生 CLI 語法。README.md 裡的使用範例(`dpm install foo` 等)在開發時等價於 `just run-client install foo`——差別只在前者跑已安裝的 binary,後者從原始碼現編現跑。

### 文件與維護

| 指令              | 說明                                       |
| ----------------- | ------------------------------------------ |
| `just doc`      | `cargo doc --workspace --no-deps --open` |
| `just clean`    | `cargo clean`                            |
| `just outdated` | 檢查過期 dependency(需`cargo-outdated`)  |
| `just audit`    | 檢查安全性漏洞(需`cargo-audit`)          |
| `just update`   | 更新`Cargo.lock`                         |

### 安裝(從原始碼)

```bash
just install-client   # cargo install --path crates/dpm,裝到 ~/.cargo/bin
just install-server   # cargo install --path crates/dpm-server
```

一般使用者不需要原始碼、不需要 Rust 工具鏈——見 README.md 的 curl 安裝腳本。這裡的 `install-client`/`install-server` 是給要在本機用開發中版本測試的人用的。

### Secrets(Infisical)

| 指令                             | 說明                                         |
| -------------------------------- | -------------------------------------------- |
| `just env-login`               | 互動登入(每台機器一次)                       |
| `just env-init`                | 建立/連結`.infisical.json`(每個 repo 一次) |
| `just env-list`                | 列出目前 environment 的 secret key(不印值)   |
| `just env-push <dotenv> <env>` | 批次匯入既有 dotenv 檔案到指定 environment   |

## 提交前

至少跑一次 `just pre-commit`(fmt + clippy + test)。

## Spec-driven workflow

本專案用 **Superpowers** skill 套件做變更管理:`brainstorming` → `writing-plans` → `executing-plans` / `subagent-driven-development`。

- Design doc:`docs/superpowers/specs/YYYY-MM-DD-<topic>-design.md`
- Implementation plan:`docs/superpowers/plans/YYYY-MM-DD-<change-name>.md`
