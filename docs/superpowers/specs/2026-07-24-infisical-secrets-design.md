# Infisical 導入設計

日期:2026-07-24

## 背景與動機

DPM-Workspace 目前沒有集中式 secret 管理。唯一存在的敏感/環境設定是 `crates/dpm/.env` 裡的 `DATABASE_URL=./LocalRepo.db`(diesel CLI 用,已被 `.gitignore` 擋掉,不會進版控)。

隨著以下需求出現,需要導入 Infisical 做集中管理:
- CI/CD 需要 GitHub token / crates.io token 來 publish、建 release、操作 repo。
- 未來 `dpm-server` 若接雲端/簽章服務,會有 server 端服務金鑰。
- 使用者其他專案已統一用 Infisical 管理 secret,DPM-Workspace 要跟上同一套流程,降低跨專案認知負擔。

## 目標

- 建立 Infisical project,三個 environment:`dev` / `staging` / `prod`。
- justfile 全部 recipe 透過統一 helper 注入 secret,secret 不落地到硬碟。
- 退役 `crates/dpm/.env`,DATABASE_URL 改由 Infisical `dev` environment 提供。
- 更新專案 CLAUDE.md,記錄 Infisical 使用方式。

## 非目標

- 不在這次導入實際填入真實的 GitHub token / crates.io token 內容(由使用者自行在 Infisical dashboard 填,Claude 不經手真實密鑰值)。
- 不建立 CI/CD pipeline 本身(GitHub Actions workflow 等),只準備好 Infisical 這一層,讓之後接 CI 時可以直接用 Machine Identity token。
- 不引入 SOPS 或 `envs/*.json` 這類舊方案。

## 架構

### Environment 模型

三個 slug:`dev` / `staging` / `prod`。專案內部若用 `ENV=test` 這個名稱,對應到 Infisical 的 `staging` slug(沿用使用者其他專案已有的慣例,維持全專案一致)。

### 本地開發認證

互動 OAuth,執行一次 `just env-login`(每台機器一次)。`.infisical.json` 記錄 repo 與既有 Infisical org 下 `DPM-Workspace` project 的連結,透過 `just env-init` 產生(連到使用者既有 org,不新建 org)。

### CI/CD 認證

Machine Identity token,經環境變數 `INFISICAL_TOKEN` 注入,不走互動登入。

### justfile 改動

新增私有 helper:

```
_run env cmd:
    infisical run --env={{env}} --path=/ --command="{{cmd}}"
```

現有全部 18 個 recipe(`check`/`build`/`release`/`test`/`test-p`/`lint`/`lint-fix`/`fmt`/`fmt-check`/`pre-commit`/`run-client`/`run-server`/`migration-new`/`migration-run`/`migration-redo`/`doc`/`clean`/`outdated`/`audit`/`update`/`install-client`/`install-server`)本體都改成透過 `_run` 執行,預設 `env=dev`,可用 `just check env=staging` 覆寫。

新增管理 recipe:
- `env-login` — 互動 OAuth 登入
- `env-init` — 產生/連結 `.infisical.json`
- `env-list` — 列出目前 secret(不印值)
- `env-push <dotenv-file> <env>` — 批次匯入既有 dotenv 檔案內容到指定 environment

### Secrets 遷移

- `DATABASE_URL` 搬進 Infisical `dev` environment,`crates/dpm/.env` 檔案本身刪除。diesel CLI 直接讀 process env,`infisical run` 注入後不需要 dotenv 這層,`dotenv = "0.15.0"` 這個 dependency(`crates/dpm/Cargo.toml`)若專案程式碼本身沒有實際呼叫 `dotenv::dotenv()`,順手確認是否還有用到,沒用到不在此次範圍內移除(避免範圍蔓延,留給之後 lint 順手處理)。
- `GITHUB_TOKEN`、`CRATES_IO_TOKEN`、未來 server 端金鑰:在 Infisical 建 key 佔位,值由使用者自行填。

### CLAUDE.md 更新

在專案 CLAUDE.md 新增「Secrets (Infisical)」段落,套用使用者提供文字並依本專案調整:三個 environment、`just _run` 包全部 recipe、`.env`/SOPS/`envs/*.json` 已退役、CI 用 Machine Identity token。

## 資料流

1. 開發者跑 `just <recipe>` → justfile 呼叫 `_run` → `infisical run --env=dev ...` 向 Infisical 拉當前 secret → 注入子行程環境變數 → 執行實際指令(cargo/diesel/…)。
2. Secret 全程只存在於行程記憶體與 Infisical 端,不寫入專案內任何檔案。
3. CI 情境下 `INFISICAL_TOKEN` 由 CI 平台的 secret 機制提供,`infisical run` 用該 token 做非互動認證,流程同上。

## 錯誤處理

- 未登入/`.infisical.json` 未連結時執行任何 recipe:`infisical run` 會回傳非 0 exit code 並印出登入提示,`just` 直接把這個 exit code 往外傳,不額外包裝訊息(避免蓋掉 Infisical 原生錯誤訊息)。
- Environment 不存在或無權限:同樣直接透傳 Infisical CLI 的錯誤訊息。

## 驗證方式

- `just env-login` 完成後跑 `just check`:確認指令正常執行且沒有因為缺 secret 而失敗。
- `just migration-run`:確認 `DATABASE_URL` 有從 Infisical `dev` environment 注入,diesel 能連到 DB。
- 確認 `crates/dpm/.env` 已刪除,`git status` 除了 `.infisical.json`、justfile、CLAUDE.md 沒有其他非預期變更。
- `just env-list` 能列出剛遷移進去的 `DATABASE_URL` 這個 key。
