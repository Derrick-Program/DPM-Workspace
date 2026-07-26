# dpm 自我更新(upgrade-self)Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 把 `ActionInfo::upgrade_self()`(目前是 `crates/dpm/src/action.rs:442` 的空殼,只印一行字)換成用 `self_update` v0.44.0 crate 真的做「查 GitHub Release → 下載對應平台 archive → 用 zipsign(ed25519)驗證簽章 → 原地換裝」,並且新增 `.github/workflows/release.yml`,讓 push `v*.*.*` tag 時自動編出 4 個平台的已簽章 `dpm` binary 發到 GitHub Release——沒有這個 workflow,`upgrade_self()` 永遠沒有東西可以抓。

**Architecture:** 六個循序 task。Task 1 是一次性、必須由人手動執行的簽章金鑰 bootstrap(產生 zipsign keypair,公鑰進 repo、私鑰進 GitHub secret)——這是 Task 3 能編譯過的前提(`include_bytes!` 讀的檔案要先存在)。Task 2 加新依賴 + 寫兩個純函式錯誤分類 helper(`is_signature_error`/`is_permission_denied`)並用 TDD 補測試,這兩個 helper 不依賴金鑰檔,可以跟 Task 1 平行進行。Task 3 是核心:把 `upgrade_self()` 從空殼換成真邏輯,呼叫端 `lib.rs` 補 `?`。Task 4 新增 release workflow。Task 5 是整個 workspace 的收尾驗證(`cargo fmt`/`clippy`/`test`)。Task 6 是純手動的端對端驗證(真的打 tag、真的跑一次 release、真的驗證簽章不符會被擋)——這部分沒有辦法被自動化測試涵蓋,必須有人實際跑過。

**Tech Stack:** Rust 2021、`self_update = "0.44.0"`(features: `archive-tar`, `compression-flate2`, `signatures`)、`git2 = "0.18.1"`(新增 `vendored-openssl`/`vendored-libgit2` features)、zipsign(ed25519,透過 `self_update` 的 `signatures` feature 使用,不是獨立依賴)、GitHub Actions(`taiki-e/upload-rust-binary-action@v1` + `gh` CLI)。

## Global Constraints

- `self_update`/`git2` 的版本與 feature 名稱以 spec(`docs/superpowers/specs/2026-07-26-self-update-design.md`)為準,不要自行加碼或猜測其他 feature——`self_update` 只開 `archive-tar`/`compression-flate2`/`signatures` 三個,`git2` 只加 `vendored-openssl`/`vendored-libgit2` 兩個 feature,版本號本身不變(`git2` 維持 `0.18.1`)。
- `self_update` 跟這次新加的 `git2` features **只寫在 `crates/dpm/Cargo.toml`**,不進根 `Cargo.toml` 的 `[workspace.dependencies]`——理由:`self_update` 是 `dpm`(client 自我更新)專用的,`dpm-server` 沒有自我更新需求；`git2` 本來就已經是只在 `crates/dpm/Cargo.toml` 裡宣告的 crate-local 依賴(不在 workspace deps 裡),這次只是幫既有宣告加 feature,延續同一個放置慣例。
- `upgrade_self()` 的簽名從 `pub fn upgrade_self(&self)`(無回傳值)改成 `pub fn upgrade_self(&self) -> ClientResult<()>`——呼叫端 `crates/dpm/src/lib.rs:88-90` 要跟著補 `?`,不能漏改。
- 測試一律用專案既有慣例:同檔案底部 `#[cfg(test)] mod xxx_tests { use super::*; ... }`(參考 `crates/dpm/src/utils/system.rs` 底部 `mod tests` 與 `crates/dpm/src/action.rs` 底部 `mod installed_package_names_tests` 的既有寫法),不要另開檔案。
- 不寫任何打真實 GitHub API 的自動化測試(跟 `crates/dpm-core/tests/test.rs::test_from_url` 同一類問題,這次不重蹈覆轍)——`self_update` 互動的驗證只能靠 Task 6 的人工步驟。
- 每個有程式碼變動的 task 結束前都要跑過 `cargo check`/`cargo clippy -- -D warnings`/相關 `cargo test`,不要留到最後一次總跑;commit message 用 Conventional Commits(`type(scope): description`)格式。
- 提交前完整跑一次 `just pre-commit`(fmt + clippy + test)是 Task 5 的內容，但個別 task 裡的中途檢查不必每次都透過 `just`／`infisical`——直接下 `cargo fmt`/`cargo clippy`/`cargo test` 對應指令即可，效果相同、不需要 Infisical session。

---

## Task 1: 一次性 zipsign 簽章金鑰 bootstrap(人工執行，非自動化)

**這個 task 不能由自動執行 plan 的 agent 自己完成**——它會產生真正的 ed25519 私鑰並寫入這個 repo 的 GitHub Actions secret，屬於敏感的一次性人工動作（spec 的「非目標」明確排除 CI 自動產生金鑰）。如果你是正在照這份 plan 執行的 agent：把下面的指令原樣列給使用者，請他們在自己的機器上執行，並在他們確認「`crates/dpm/keys/dpm-release-signing.pub` 已經產生且 commit、GitHub secret `ZIPSIGN_SIGNING_KEY_B64` 已經設定」之後，才可以開始 Task 3（Task 2 不依賴這把金鑰，可以先做）。

**Files:**
- Create: `crates/dpm/keys/dpm-release-signing.pub`(32 bytes，commit 進 repo)
- Modify: `.gitignore`(新增一條明確擋 `dpm-release-signing.priv`)

**Interfaces:**
- Produces: `crates/dpm/keys/dpm-release-signing.pub`——Task 3 的 `include_bytes!("../keys/dpm-release-signing.pub")` 直接讀這個檔案，編譯期就會失敗如果它不存在或不是剛好 32 bytes。
- Produces: GitHub repo 的 Actions secret `ZIPSIGN_SIGNING_KEY_B64`——Task 4 的 `.github/workflows/release.yml` 讀這個 secret。

- [ ] **Step 1: 安裝 zipsign CLI**

在你自己的機器上（不是 CI）執行：

```bash
cargo install zipsign --locked
```

- [ ] **Step 2: 產生 keypair**

```bash
cd /tmp
zipsign gen-key dpm-release-signing.priv dpm-release-signing.pub
```

預期產出兩個檔案：`dpm-release-signing.priv`(64 bytes)、`dpm-release-signing.pub`(32 bytes)。用 `wc -c` 確認大小：

```bash
wc -c dpm-release-signing.priv dpm-release-signing.pub
```

Expected: `64 dpm-release-signing.priv`、`32 dpm-release-signing.pub`。

- [ ] **Step 3: 公鑰進 repo**

```bash
mkdir -p /path/to/DPM-Workspace/crates/dpm/keys
cp /tmp/dpm-release-signing.pub /path/to/DPM-Workspace/crates/dpm/keys/dpm-release-signing.pub
```

- [ ] **Step 4: `.gitignore` 明確擋私鑰檔名**

編輯 repo 根目錄 `.gitignore`，在既有的 `*.key` 那條規則附近新增一行：

```
dpm-release-signing.priv
```

（現有的 `*.key` 規則是防一般用途金鑰檔，不保證涵蓋這裡刻意挑的 `.priv` 副檔名——見 spec「架構」段落的說明，這條規則獨立補上，不依賴 `*.key` 剛好命中。）

- [ ] **Step 5: 私鑰轉 base64、存進 GitHub secret**

```bash
base64 -i /tmp/dpm-release-signing.priv | tr -d '\n' > /tmp/dpm-release-signing.priv.b64
gh secret set ZIPSIGN_SIGNING_KEY_B64 \
  --repo Derrick-Program/DPM-Workspace \
  < /tmp/dpm-release-signing.priv.b64
```

（沒有裝 `gh` CLI 或想用網頁介面也可以：GitHub repo → Settings → Secrets and variables → Actions → New repository secret，name 填 `ZIPSIGN_SIGNING_KEY_B64`，value 貼 `/tmp/dpm-release-signing.priv.b64` 的內容。）

確認 secret 已經設定成功：

```bash
gh secret list --repo Derrick-Program/DPM-Workspace | grep ZIPSIGN_SIGNING_KEY_B64
```

Expected: 印出一行 `ZIPSIGN_SIGNING_KEY_B64  <updated-time>`。

- [ ] **Step 6: 清掉本機的私鑰檔**

```bash
rm -f /tmp/dpm-release-signing.priv /tmp/dpm-release-signing.priv.b64
```

（`dpm-release-signing.pub` 不用刪，它已經 commit 進 repo 了；`/tmp` 底下留著的私鑰明文檔案沒有理由繼續存在。）

- [ ] **Step 7: Commit 公鑰 + gitignore 改動**

```bash
cd /path/to/DPM-Workspace
git add crates/dpm/keys/dpm-release-signing.pub .gitignore
git commit -m "$(cat <<'EOF'
chore(dpm): add zipsign release-signing public key

One-time bootstrap: the ed25519 keypair used to sign/verify dpm
release binaries. Public key (32 bytes) committed here; paired
private key (64 bytes) lives only in the GitHub Actions secret
ZIPSIGN_SIGNING_KEY_B64, never in this repo. .gitignore gets an
explicit dpm-release-signing.priv entry as a backstop in case anyone
generates the keypair inside the repo by mistake.

Co-Authored-By: Claude Sonnet 5 <noreply@anthropic.com>
EOF
)"
```

---

## Task 2: 加新依賴 + 錯誤分類 helper(`is_signature_error`/`is_permission_denied`)

**Files:**
- Modify: `crates/dpm/Cargo.toml`
- Modify: `crates/dpm/src/action.rs`

**Interfaces:**
- Consumes: 無（純新增）。
- Produces: `fn is_signature_error(e: &self_update::errors::Error) -> bool`、`fn is_permission_denied(e: &self_update::errors::Error) -> bool`——兩個 module-level 私有函式，定義在 `crates/dpm/src/action.rs`，Task 3 的 `upgrade_self()` 會直接呼叫。

- [ ] **Step 1: `Cargo.toml` 加新依賴**

編輯 `crates/dpm/Cargo.toml`，把現有的：

```toml
git2 = "0.18.1"
```

改成：

```toml
git2 = { version = "0.18.1", features = ["vendored-openssl", "vendored-libgit2"] }
```

並在 `[dependencies]` 區塊新增一行：

```toml
self_update = { version = "0.44.0", features = ["archive-tar", "compression-flate2", "signatures"] }
```

（`vendored-openssl`/`vendored-libgit2` 讓 Task 4 的 CI cross-compile 不用依賴 runner 上的系統 libgit2/openssl；本機第一次 `cargo check` 因為要從原始碼編 openssl 會比平常慢一點，這是預期中的副作用，不是錯誤。）

- [ ] **Step 2: 確認新依賴能解析**

Run: `cargo check -p DPM`
Expected: 編譯成功（`self_update`/新 `git2` features 目前都還沒被程式碼用到，只是依賴解析，不會報錯；vendored openssl 首次編譯需要多等一陣子）。

- [ ] **Step 3: 寫失敗的測試（TDD——函式還不存在）**

在 `crates/dpm/src/action.rs` 檔案最底部（`installed_package_names_tests` mod 的 `}` 之後）新增：

```rust
#[cfg(test)]
mod upgrade_self_tests {
    use super::*;
    use self_update::errors::Error;
    use std::io;

    #[test]
    fn permission_denied_io_error_is_detected() {
        let err = Error::Io(io::Error::new(io::ErrorKind::PermissionDenied, "denied"));
        assert!(is_permission_denied(&err));
    }

    #[test]
    fn not_found_io_error_is_not_permission_denied() {
        let err = Error::Io(io::Error::new(io::ErrorKind::NotFound, "missing"));
        assert!(!is_permission_denied(&err));
    }

    #[test]
    fn non_io_error_is_not_permission_denied() {
        let err = Error::Network("boom".to_string());
        assert!(!is_permission_denied(&err));
    }

    #[test]
    fn no_signatures_error_is_a_signature_error() {
        let err = Error::NoSignatures(self_update::ArchiveKind::Tar(None));
        assert!(is_signature_error(&err));
    }

    #[test]
    fn non_signature_error_is_not_a_signature_error() {
        let err = Error::Network("boom".to_string());
        assert!(!is_signature_error(&err));
    }

    // `Error::Signature(zipsign_api::ZipsignError)` isn't covered by its own
    // fixture here: `zipsign_api` is only a transitive dependency (pulled in
    // by self_update's `signatures` feature) and isn't re-exported by
    // self_update, so building one would mean adding zipsign_api as a direct
    // dev-dependency solely to construct a test value. The `matches!` arm in
    // `is_signature_error` covers both `NoSignatures` and `Signature` as one
    // pattern — `no_signatures_error_is_a_signature_error` above exercises
    // that same arm.
}
```

- [ ] **Step 4: 跑測試,確認因為函式不存在而編譯失敗**

Run: `cargo test -p DPM upgrade_self_tests`
Expected: 編譯失敗,錯誤訊息包含 `cannot find function `is_permission_denied`` 和/或 `cannot find function `is_signature_error``。

- [ ] **Step 5: 實作兩個 helper 函式**

在 `crates/dpm/src/action.rs` 裡,找到 `impl ActionInfo { ... }` 區塊結束的 `}`(第 445 行附近,`upgrade_self` 目前的空殼就在這個 impl 裡)之後、`installed_package_names` 函式之前,插入:

```rust
/// `self_update::errors::Error::NoSignatures`/`Error::Signature` 都代表下載
/// 回來的 release archive 沒有通過 `RELEASE_SIGNING_PUBLIC_KEY` 的 zipsign
/// 驗證——這是安全相關的失敗,`upgrade_self` 會給它一個獨立的 `INSECURE:`
/// 開頭訊息,跟一般網路/查詢錯誤分開處理。
fn is_signature_error(e: &self_update::errors::Error) -> bool {
    matches!(
        e,
        self_update::errors::Error::NoSignatures(_) | self_update::errors::Error::Signature(_)
    )
}

/// `self_update::errors::Error::Io` 底下包著
/// `io::ErrorKind::PermissionDenied` 代表目前使用者對 `dpm` 執行檔所在位置
/// 沒有寫入權限(例如執行檔是被其他使用者以 system-wide 方式裝的)。
/// `upgrade_self` 會在這個錯誤後面補一句 `sudo dpm upgrade-self` 提示,
/// 而不是直接印出原始的 OS 錯誤訊息。
fn is_permission_denied(e: &self_update::errors::Error) -> bool {
    matches!(
        e,
        self_update::errors::Error::Io(io_err)
            if io_err.kind() == std::io::ErrorKind::PermissionDenied
    )
}
```

- [ ] **Step 6: 跑測試,確認通過**

Run: `cargo test -p DPM upgrade_self_tests`
Expected: 5 個測試全部 PASS(`permission_denied_io_error_is_detected`、`not_found_io_error_is_not_permission_denied`、`non_io_error_is_not_permission_denied`、`no_signatures_error_is_a_signature_error`、`non_signature_error_is_not_a_signature_error`)。

- [ ] **Step 7: clippy**

Run: `cargo clippy -p DPM --all-targets -- -D warnings`
Expected: 無警告無錯誤。

- [ ] **Step 8: Commit**

```bash
git add crates/dpm/Cargo.toml crates/dpm/src/action.rs
git commit -m "$(cat <<'EOF'
feat(dpm): add self_update/git2-vendored deps + error classifiers

self_update 0.44.0 (archive-tar, compression-flate2, signatures) is
the crate upgrade_self() will use next. git2 gets vendored-openssl/
vendored-libgit2 so the upcoming release CI can cross-compile without
relying on the runner's system libgit2/openssl.

is_signature_error/is_permission_denied classify self_update's error
enum into the two cases upgrade_self needs to message differently:
a tampered/unsigned download (INSECURE, refuse to install) vs. a
non-writable binary path (hint to re-run with sudo).

Co-Authored-By: Claude Sonnet 5 <noreply@anthropic.com>
EOF
)"
```

---

## Task 3: 實作 `ActionInfo::upgrade_self()`

**依賴 Task 1**:這個 task 的 Step 4(`cargo check`)需要 `crates/dpm/keys/dpm-release-signing.pub` 已經存在(`include_bytes!` 是編譯期讀檔)。如果 Task 1 的人工步驟還沒完成,先停在這裡等待。

**Files:**
- Modify: `crates/dpm/src/action.rs`
- Modify: `crates/dpm/src/lib.rs:88-90`

**Interfaces:**
- Consumes: `is_signature_error`、`is_permission_denied`(Task 2)、`ClientError::SystemError`(既有,`crates/dpm/src/utils/error.rs`)、`self_update::backends::github::Update`、`self_update::Status`。
- Produces: `pub fn upgrade_self(&self) -> ClientResult<()>`——簽名從無回傳值改成 `ClientResult<()>`,`lib.rs::entry()` 的呼叫端要補 `?`。

- [ ] **Step 1: 在 `impl ActionInfo` 之前加簽章公鑰常數**

編輯 `crates/dpm/src/action.rs`,在 `type ParsedInstallSpec = ...;` 這行之後、`#[derive(Debug)] pub struct ActionInfo { ... }` 之前,插入:

```rust
/// 驗證下載回來的 `dpm` release archive 簽章用的 ed25519 公鑰(32 bytes)。
/// 對應的私鑰不會出現在這個 repo 裡——它只存在 GitHub Actions secret
/// `ZIPSIGN_SIGNING_KEY_B64`,由 `.github/workflows/release.yml` 用來簽每個
/// release asset。金鑰產生方式見
/// `docs/superpowers/specs/2026-07-26-self-update-design.md`「簽章金鑰」段落。
const RELEASE_SIGNING_PUBLIC_KEY: &[u8; 32] = include_bytes!("../keys/dpm-release-signing.pub");
```

- [ ] **Step 2: 換掉 `upgrade_self` 的空殼實作**

在同一個檔案裡,把:

```rust
    pub fn upgrade_self(&self) {
        println!("{} Upgrading self", "==>".blue());
    }
```

換成:

```rust
    pub fn upgrade_self(&self) -> ClientResult<()> {
        let status = self_update::backends::github::Update::configure()
            .repo_owner("Derrick-Program")
            .repo_name("DPM-Workspace")
            .bin_name("dpm")
            .show_download_progress(self.verbose)
            .show_output(self.verbose)
            .current_version(env!("CARGO_PKG_VERSION"))
            .verifying_keys([*RELEASE_SIGNING_PUBLIC_KEY])
            .build()
            .map_err(|e| {
                ClientError::SystemError(format!("failed to configure self-update: {e}"))
            })?
            .update();

        match status {
            Ok(self_update::Status::UpToDate(v)) => {
                println!("{} dpm is already up to date (v{v})", "==>".green());
            }
            Ok(self_update::Status::Updated(v)) => {
                println!("{} dpm updated to v{v}", "==>".green());
            }
            Err(e) if is_signature_error(&e) => {
                return Err(ClientError::SystemError(format!(
                    "{}\n{} downloaded update failed signature verification — refusing to install. \
                     This could mean the release was tampered with, or is missing a valid signature. \
                     Not proceeding.",
                    e,
                    "INSECURE:".red().bold()
                )));
            }
            Err(e) if is_permission_denied(&e) => {
                return Err(ClientError::SystemError(format!(
                    "{e}\nhint: dpm's binary isn't writable by the current user — try `sudo dpm upgrade-self`"
                )));
            }
            Err(e) => return Err(ClientError::SystemError(e.to_string())),
        }
        Ok(())
    }
```

（`repo_owner`/`repo_name`/`bin_name` 是編譯期常數,直接寫死,不透過 config——這是 `dpm` 更新自己專用的邏輯,不是使用者可設定的套件來源。`"==>".blue()` 那行不再使用,`colored::Colorize` 的 `.green()`/`.red()`/`.bold()` 仍然透過檔案開頭既有的 `use colored::Colorize;` 取得。）

- [ ] **Step 3: 更新 `lib.rs` 呼叫端**

編輯 `crates/dpm/src/lib.rs`,把第 88-90 行:

```rust
        Some(Commands::UpgradeSelf { verbose }) => {
            ActionInfo::new(ctx.clone(), vec![], verbose, setting_config).upgrade_self()
        }
```

改成:

```rust
        Some(Commands::UpgradeSelf { verbose }) => {
            ActionInfo::new(ctx.clone(), vec![], verbose, setting_config).upgrade_self()?
        }
```

- [ ] **Step 4: 確認整個 crate 能編譯**

Run: `cargo check -p DPM`
Expected: 編譯成功。**如果這裡因為 `include_bytes!` 找不到 `crates/dpm/keys/dpm-release-signing.pub` 而失敗**,代表 Task 1 還沒做完——回去完成 Task 1 的 Step 1-3 再繼續。

- [ ] **Step 5: clippy**

Run: `cargo clippy -p DPM --all-targets -- -D warnings`
Expected: 無警告無錯誤。

- [ ] **Step 6: 既有測試沒有回歸**

Run: `cargo test -p DPM`
Expected: 全部通過(含 Task 2 新增的 5 個 `upgrade_self_tests`,以及既有的 `installed_package_names_tests`/`system.rs` 底下的 `tests` 等)。

- [ ] **Step 7: 手動 smoke test(此時還沒有真的 GitHub Release,預期會報網路/找不到 release 的錯誤,不是 panic)**

Run: `cargo run -p DPM -- upgrade-self --verbose`
Expected: 不會 panic、不會 `.unwrap()` 炸掉。在 Task 4/6 完成、真的有 release 之前,合理的結果是印出一個由 `ClientError::SystemError` 包起來、來自 GitHub API 的錯誤(例如找不到任何 release),透過 `main.rs` 既有的錯誤路徑印出並以非 0 結束——這才是這次要驗證的行為:錯誤會清楚地往上傳,而不是被吞掉或讓程式崩潰。

- [ ] **Step 8: Commit**

```bash
git add crates/dpm/src/action.rs crates/dpm/src/lib.rs
git commit -m "$(cat <<'EOF'
feat(dpm): implement upgrade_self() via self_update + zipsign verify

Replaces the upgrade_self() placeholder (println! only, no Result)
with a real self_update::backends::github::Update flow: query the
GitHub Release API, download the matching-target archive, verify its
zipsign signature against the committed public key, and swap the
running binary in place. Signature failures (NoSignatures/Signature)
get a distinct INSECURE-prefixed message and refuse to install;
permission-denied failures get a `sudo dpm upgrade-self` hint.
Return type changes from () to ClientResult<()> — lib.rs's call site
now propagates with `?` instead of swallowing every possible failure.

Co-Authored-By: Claude Sonnet 5 <noreply@anthropic.com>
EOF
)"
```

---

## Task 4: 新增 `.github/workflows/release.yml`

**Files:**
- Create: `.github/workflows/release.yml`

**Interfaces:**
- Consumes: GitHub secret `ZIPSIGN_SIGNING_KEY_B64`(Task 1)。
- Produces: 4 個已簽章的 `dpm-<target>.tar.gz` release asset,檔名裡的 target triple 是 `self_update::get_target()` 用來比對的字串,必須跟 Rust 官方 target 名稱完全一致(`x86_64-apple-darwin`/`aarch64-apple-darwin`/`x86_64-unknown-linux-gnu`/`aarch64-unknown-linux-gnu`)。

- [ ] **Step 1: 寫 workflow 檔**

建立 `.github/workflows/release.yml`:

```yaml
name: Release

on:
  push:
    tags:
      - "v*.*.*"

permissions:
  contents: write

jobs:
  create-release:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4

      - name: Create the GitHub Release for this tag (idempotent)
        env:
          GH_TOKEN: ${{ secrets.GITHUB_TOKEN }}
        run: |
          set -euo pipefail
          if gh release view "${{ github.ref_name }}" >/dev/null 2>&1; then
            echo "release ${{ github.ref_name }} already exists, reusing it"
          else
            gh release create "${{ github.ref_name }}" --title "${{ github.ref_name }}" --generate-notes
          fi

  upload-assets:
    needs: create-release
    strategy:
      fail-fast: false
      matrix:
        include:
          - target: x86_64-apple-darwin
            os: macos-latest
          - target: aarch64-apple-darwin
            os: macos-latest
          - target: x86_64-unknown-linux-gnu
            os: ubuntu-latest
          - target: aarch64-unknown-linux-gnu
            os: ubuntu-latest
    runs-on: ${{ matrix.os }}
    steps:
      - uses: actions/checkout@v4

      - name: Build and package dpm for ${{ matrix.target }} (no upload yet)
        id: package
        uses: taiki-e/upload-rust-binary-action@v1
        with:
          bin: dpm
          target: ${{ matrix.target }}
          tar: unix
          dry-run: true
          token: ${{ secrets.GITHUB_TOKEN }}

      - name: Install zipsign
        run: cargo install zipsign --locked

      - name: Decode the release signing private key
        env:
          ZIPSIGN_SIGNING_KEY_B64: ${{ secrets.ZIPSIGN_SIGNING_KEY_B64 }}
        run: echo "$ZIPSIGN_SIGNING_KEY_B64" | base64 -d > dpm-release-signing.priv

      - name: Sign the packaged archive
        run: zipsign sign tar "${{ steps.package.outputs.tar }}" dpm-release-signing.priv

      - name: Remove the signing key from the runner
        if: always()
        run: rm -f dpm-release-signing.priv

      - name: Upload the signed archive to the release
        env:
          GH_TOKEN: ${{ secrets.GITHUB_TOKEN }}
        run: gh release upload "${{ github.ref_name }}" "${{ steps.package.outputs.tar }}" --clobber
```

（`create-release` 先確保這個 tag 有一個可以上傳 asset 的 Release——`taiki-e/upload-rust-binary-action` 在 `dry-run: true` 下只編譯+打包,完全不碰 Release,所以上傳這一步一定要有一個已存在的 Release 可以掛。`upload-assets` 用 `needs: create-release` 確保順序。`dry-run: true` 讓簽章這一步能插在「打包完」跟「上傳」中間;`steps.package.outputs.tar` 是這個 action 在 `tar: unix` 下產生的 `.tar.gz` 檔名(`$bin-$target.tar.gz` 命名規則)。cross-compile `aarch64-unknown-linux-gnu` 由這個 action 內建的 `cross` 處理,`git2` 在 Task 2 加的 `vendored-openssl`/`vendored-libgit2` features 讓這個 cross 容器內建置不用管系統依賴。`zipsign sign tar` 是原地簽,簽章直接附加進同一個 `.tar.gz`,不產生額外檔案。私鑰檔用完立刻 `rm -f`(搭配 `if: always()`,即使前面步驟失敗也會清)。只建 `dpm`,不含 `dpm-server`——後者沒有自我更新需求。）

- [ ] **Step 2: YAML 語法檢查**

Run: `python3 -c "import yaml; yaml.safe_load(open('.github/workflows/release.yml')); print('OK')"`
Expected: 印出 `OK`,不拋出例外。

（這個 workflow 真正的正確性——4 個平台都編得過、簽章有效、`self_update` 真的抓得到——沒辦法在這個 task 裡自動化驗證,需要真的打一個 tag 觸發一次 run,見 Task 6。）

- [ ] **Step 3: Commit**

```bash
git add .github/workflows/release.yml
git commit -m "$(cat <<'EOF'
ci: add signed multi-platform release workflow

Triggered by pushing a v*.*.* tag (the existing `just tag-release`
convention). create-release ensures the tag has a GitHub Release to
upload into; upload-assets builds+packages dpm for 4 targets (macOS
x86_64/aarch64, Linux x86_64/aarch64) via taiki-e/upload-rust-binary-
action in dry-run mode, signs each archive with zipsign using the
ZIPSIGN_SIGNING_KEY_B64 secret, then uploads the signed archive —
giving upgrade_self() something to actually download and verify.

Co-Authored-By: Claude Sonnet 5 <noreply@anthropic.com>
EOF
)"
```

---

## Task 5: 整個 workspace 收尾驗證

**Files:** 無新增/修改(純驗證)。

**Interfaces:** 無。

- [ ] **Step 1: `cargo check --workspace` 通過(spec 驗證清單第一條)**

Run: `cargo check --workspace`
Expected: 編譯成功,無錯誤。

- [ ] **Step 2: 格式化檢查**

Run: `cargo fmt --all -- --check`
Expected: 無輸出(代表已經是格式化過的狀態)。如果有輸出,跑 `cargo fmt --all` 再重新檢查一次。

- [ ] **Step 3: clippy(整個 workspace)**

Run: `cargo clippy --workspace --all-targets -- -D warnings`
Expected: 無警告無錯誤。

- [ ] **Step 4: 整個 workspace 測試**

Run: `cargo test --workspace`
Expected: 全部通過,包含 `DPM` crate 新增的 `upgrade_self_tests`(5 個)以及 `dpm-core`/`dpm-server` 既有測試。

- [ ] **Step 5: 確認沒有漏 commit 的變動**

Run: `git status`
Expected: working tree clean(Task 1-4 每個都已經各自 commit 過)。如果有殘留變動,補一個對應的 commit。

- [ ] **Step 6(可選,若已設定 Infisical)：用專案慣用的 `just pre-commit` 再跑一次同一組檢查**

Run: `just pre-commit`
Expected: fmt + lint + test 三步都通過(跟 Step 1-3 邏輯上是同一件事,只是走專案慣用的 wrapper)。沒有 `infisical` session 的話跳過這步,Step 1-3 已經涵蓋。

---

## Task 6: 手動端對端驗證(人工執行，需要在 Task 1-5 都合併/發布之後）

**這個 task 全部是人工步驟**,原因跟 Task 1 一樣:需要真實的 GitHub tag push、真實跑一次 Actions、在真的機器上跑舊版 `dpm` 去更新——這些沒有辦法被自動執行 plan 的 agent 在沙盒裡完成。對應 spec「驗證清單」的最後 3 條與「測試計畫」的最後兩條。

**Files:** 無(不改程式碼)。

**Interfaces:** 無。

- [ ] **Step 1: 確認 Task 1 的產出還在**

```bash
git log --oneline -- crates/dpm/keys/dpm-release-signing.pub
gh secret list --repo Derrick-Program/DPM-Workspace | grep ZIPSIGN_SIGNING_KEY_B64
```

Expected: 兩個都有東西。

- [ ] **Step 2: 打 tag、push**

```bash
just version-set <新版本號,例如 0.1.3>
just tag-release
git push origin v<新版本號>
```

- [ ] **Step 3: 觀察 release workflow 跑完,確認 4 個已簽章 asset 出現**

到 GitHub repo 的 Actions 分頁確認 `Release` workflow 綠燈跑完,再到 Releases 頁面確認這個 tag 底下有 4 個檔案:`dpm-x86_64-apple-darwin.tar.gz`、`dpm-aarch64-apple-darwin.tar.gz`、`dpm-x86_64-unknown-linux-gnu.tar.gz`、`dpm-aarch64-unknown-linux-gnu.tar.gz`。

- [ ] **Step 4: 本機用 zipsign 驗證簽章有效**

下載其中一個 asset(以 `x86_64-unknown-linux-gnu` 為例),在本機驗證:

```bash
curl -LO https://github.com/Derrick-Program/DPM-Workspace/releases/download/v<新版本號>/dpm-x86_64-unknown-linux-gnu.tar.gz
zipsign verify tar dpm-x86_64-unknown-linux-gnu.tar.gz crates/dpm/keys/dpm-release-signing.pub
```

Expected: 驗證成功,無錯誤。

- [ ] **Step 5: 在一台裝有舊版 `dpm` 的機器上實際跑 `upgrade-self`**

```bash
dpm upgrade-self --verbose
```

Expected: 印出 `==> dpm updated to v<新版本號>`,執行檔確實被換成新版(`dpm --version` 確認)。

- [ ] **Step 6: 驗證簽章不符時會被擋下(這是本次新增行為裡最重要的一條路徑)**

```bash
# 產生一把跟 CI 簽章不同的假 keypair
cd /tmp
zipsign gen-key fake-signing.priv fake-signing.pub

# 暫時把假公鑰換進 client 原始碼,重新編譯
cp /tmp/fake-signing.pub /path/to/DPM-Workspace/crates/dpm/keys/dpm-release-signing.pub
cd /path/to/DPM-Workspace
cargo build -p DPM --release
./target/release/dpm upgrade-self --verbose
```

Expected: 印出包含 `INSECURE:` 的訊息(「downloaded update failed signature verification — refusing to install」),指令以非 0 結束,**沒有**把新版換裝上去。

驗證完之後把改動還原,重新編譯回正常狀態:

```bash
git checkout -- crates/dpm/keys/dpm-release-signing.pub
cargo build -p DPM --release
rm -f /tmp/fake-signing.priv /tmp/fake-signing.pub
```

- [ ] **Step 7: 全部確認通過後,在對話/PR 紀錄裡記一句「Task 6 手動驗證已完成」**

沒有程式碼要 commit——這步純粹是留一個人工確認已經做過的紀錄(例如在對應的 PR 或 issue 留言),讓之後回頭查的人知道這條路徑真的被跑過,不是只看程式碼邏輯合理就假設沒問題。
