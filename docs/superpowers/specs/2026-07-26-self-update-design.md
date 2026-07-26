# dpm 自我更新設計

日期:2026-07-26

## 背景與動機

`dpm` 有三個概念上不同的「更新」,目前只有兩個有真實邏輯:

- `dpm update` — 刷新本地套件索引(從 `setting_config.sources` 重抓每個來源的 `RepoInfo.json`),已存在。
- `dpm upgrade` — 升級已裝套件到解析出的最新版本,已存在(`crates/dpm/src/action.rs::install_resolved`,`2026-07-26` 稍早的重構已經把這個從除錯輸出改成真的會呼叫解析+安裝邏輯)。
- `dpm upgrade-self` — 升級 `dpm` 這個執行檔本身,目前是空殼(`ActionInfo::upgrade_self`,`crates/dpm/src/action.rs:442`,只印一行字,沒有任何邏輯)。

這次要補的是第三個:讓 `upgrade-self` 真的用 `self_update` v0.44.0 crate 檢查 GitHub Release、下載對應平台的二進位檔、原地換裝。

`self_update` 的 GitHub backend 需要 repo 有照平台/架構分開的 release asset 可以抓,但目前 `Derrick-Program/DPM-Workspace` 沒有任何 GitHub Release,也沒有 CI 在編譯發布用的 binary(`.github/workflows/` 現有的 `publish.yml`/`pr-check.yml` 都不做這件事)。所以這次也要把「打 tag 之後編多平台 binary 並發到 GitHub Release」的 workflow 建起來,`upgrade-self` 才有東西可以抓。

## 目標

- `ActionInfo::upgrade_self()` 改成用 `self_update` v0.44.0 crate 做真的版本檢查 + 下載 + 換裝。
- 新增 `.github/workflows/release.yml`:push `v*.*.*` tag 時觸發,編 4 個平台(macOS x86_64/aarch64、Linux x86_64/aarch64)的 `dpm` binary,打包成 `self_update` 認得的 target-triple 命名格式,發到 GitHub Release。
- `crates/dpm/Cargo.toml` 的 `git2` 加上 `vendored-openssl`/`vendored-libgit2` feature,讓跨平台編譯不依賴系統 libgit2/openssl。

## 非目標

- 不處理 `dpm` 執行檔本身的權限自動提升。目前 `dpm` 是透過 `cargo install --path crates/dpm`(裝進 `~/.cargo/bin`)取得,使用者説明未來會有安裝/移除腳本把 `dpm` 放進系統 PATH(如 `/usr/local/bin`)、擁有者是 root——那種情況下升級需要寫權限,做法是使用者自己 `sudo dpm upgrade-self`,不是 dpm 自動偵測 `--system` flag 或自動 re-exec 提權。`--system` flag 本身只控制*套件*安裝位置(`ctx.install_dir`/`ctx.bin_dir`),跟 `dpm` 自己的執行檔位置無關,兩者不要混在一起處理。
- 不做「安裝/移除腳本」本身(使用者提到未來會做,但這次範圍只到 `upgrade-self` 本身能不能正常運作)。
- 不做簽章驗證/checksum pinning——`self_update` 走 GitHub Release API 原生流程,不額外接 blake3/GPG 之類的二次驗證(跟 `dpm-server`/`dpm` 對「套件」做的 hash 驗證是兩件事,dpm 自己的二進位檔升級沿用 `self_update` 預設信任模型)。
- 不改動 `dpm update`/`dpm upgrade` 既有邏輯。

## 架構

### `upgrade_self()` 實作

```rust
pub fn upgrade_self(&self) -> ClientResult<()> {
    let status = self_update::backends::github::Update::configure()
        .repo_owner("Derrick-Program")
        .repo_name("DPM-Workspace")
        .bin_name("dpm")
        .show_download_progress(self.verbose)
        .show_output(self.verbose)
        .current_version(env!("CARGO_PKG_VERSION"))
        .build()
        .map_err(|e| ClientError::SystemError(format!("failed to configure self-update: {e}")))?
        .update();

    match status {
        Ok(self_update::Status::UpToDate(v)) => {
            println!("{} dpm is already up to date (v{v})", "==>".green());
        }
        Ok(self_update::Status::Updated(v)) => {
            println!("{} dpm updated to v{v}", "==>".green());
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

- `is_permission_denied` 是個小 helper,檢查 `self_update::errors::Error::Io` 底下的 `io::ErrorKind::PermissionDenied`(唯一新增的判斷邏輯——其餘全部委派給 `self_update` crate)。
- 回傳型別從 `()` 改成 `ClientResult<()>`——目前 `upgrade_self` 簽名是 `pub fn upgrade_self(&self)`(見 `crates/dpm/src/lib.rs:88` 呼叫端 `ActionInfo::new(...).upgrade_self()`,沒有 `?`),這次一起改成回傳 `Result`,呼叫端補上 `?`,理由:`self_update` 的每一步(查詢 GitHub API、下載、解壓、換裝)都可能失敗,原本的空殼簽名沒這個問題所以看不出來,現在是時候讓錯誤能傳出去而不是 `.unwrap()` 或吞掉。
- `bin_name`/`repo_owner`/`repo_name` 三個值是編譯期常數,直接寫死在函式裡(不透過 config 檔——這是 dpm 更新自己專用的邏輯,不是使用者可設定的套件來源,沒有理由讓它可設定)。
- `current_version(env!("CARGO_PKG_VERSION"))` 讀的是編進 binary 的版本號,跟根 `Cargo.toml` 的 `[workspace.package] version` 是同一個值(`version.workspace = true` 繼承鏈)。

### `Cargo.toml` 改動

`crates/dpm/Cargo.toml`:

```toml
self_update = { version = "0.44.0", features = ["archive-tar", "compression-flate2"] }
git2 = { version = "0.18.1", features = ["vendored-openssl", "vendored-libgit2"] }
```

`self_update` 的 features 視 release 產物打包格式而定——目前規劃用 `.tar.gz`(macOS/Linux 都用同一種,不用另外處理 `.zip`),所以只開 `archive-tar`(tar 格式)+ `compression-flate2`(gzip 解壓縮)。*(初版 spec 誤寫成 `compress`,已依 docs.rs 上 0.44.0 的實際 feature 名稱修正。)*

### Release workflow(`.github/workflows/release.yml`)

觸發條件:`on: push: tags: ["v*.*.*"]`——跟現有 `just tag-release`(讀 `Cargo.toml` 版本、本地打 annotated tag,不自動 push)的既有慣例銜接,使用者流程不變:`just tag-release` → 確認 → `git push origin vX.Y.Z` → 這個 workflow 自動接手。

Job matrix(4 組,用 [`taiki-e/upload-rust-binary-action`](https://github.com/taiki-e/upload-rust-binary-action) 一次處理 cross-compile + 打包 + 上傳,不手刻 cross-compilation 邏輯):

| target | runner |
|---|---|
| `x86_64-apple-darwin` | `macos-latest` |
| `aarch64-apple-darwin` | `macos-latest` |
| `x86_64-unknown-linux-gnu` | `ubuntu-latest` |
| `aarch64-unknown-linux-gnu` | `ubuntu-latest`(action 內部用 `cross`/zig 處理 cross toolchain) |

每個 target 產出 `dpm-<target>.tar.gz`,內含 `dpm` 執行檔——`self_update` 預設用 `self_update::get_target()`(回傳目前執行環境的 Rust target triple字串)比對 release asset 檔名裡有沒有包含該字串,所以檔名裡的 target triple 一定要跟 Rust 官方 target 名稱完全一致,不能自己另外發明命名規則。

Workflow 只建 `dpm`(client)的 release,不含 `dpm-server`——`dpm-server` 目前沒有自我更新需求,不在這次範圍內。

### 資料流

```
使用者執行 dpm upgrade-self
  -> self_update 查 GitHub API:GET /repos/Derrick-Program/DPM-Workspace/releases/latest
  -> 比對 release tag(vX.Y.Z)與 current_version
  -> 相同 或 target 找不到對應 asset -> 回報並結束(Status::UpToDate 或 Err)
  -> 版本較新 -> 下載對應 target 的 dpm-<target>.tar.gz 到系統暫存目錄
  -> 解壓、取出 dpm 執行檔
  -> 原地覆蓋 current_exe()(self_update 內部用暫存檔 + rename,同檔案系統的原子替換)
  -> 回報 Status::Updated(新版本號)
```

## 錯誤處理

- GitHub API 查詢失敗(離線、rate limit)→ `self_update` 回傳 `Err`,包成 `ClientError::SystemError`,原樣往上拋,`main.rs` 既有的 `eprintln!` + `exit(1)` 路徑接手,不用新增處理。
- 目標平台沒有對應 release asset(例如某次 release 只成功發了 3 個平台)→ `self_update` 回傳 `Err`(`ReleaseNotFound`/類似錯誤),同上直接往上拋,不特別攔截。
- 寫入權限不足(執行檔所在目錄不可寫)→ 攔截 `io::ErrorKind::PermissionDenied`,額外印一行 `sudo dpm upgrade-self` 提示,不自動提權。

## 測試計畫

- `self_update` crate 本身的 GitHub API 互動不寫整合測試(需要真實網路 + 真實 GitHub release,跟 `crates/dpm-core/tests/test.rs::test_from_url` 那種打真網路的測試是同一類問題,這次不重蹈覆轍)。
- `is_permission_denied` helper 是唯一新增的判斷邏輯,寫一個小的 `#[test]` 直接建構 `io::Error::new(io::ErrorKind::PermissionDenied, "x")` 包進 `self_update::errors::Error::Io`,驗證回傳 `true`;其他 `ErrorKind`(如 `NotFound`)驗證回傳 `false`。
- `release.yml` workflow 本身用「打一個測試 tag 觸發一次真實 run,人工確認 4 個 asset 都出現在 GitHub Release 頁面」驗證,不寫自動化測試(CI workflow 正確性本來就是靠跑一次來驗證,寫 meta-test 意義不大)。

## 驗證清單

- [ ] `cargo check --workspace` 通過
- [ ] `cargo clippy --workspace --all-targets` 通過
- [ ] `cargo test --workspace` 通過(含新增的 `is_permission_denied` 測試)
- [ ] 打一個真實 tag、push、觀察 `release.yml` 跑完,GitHub Release 頁面出現 4 個 `dpm-<target>.tar.gz` asset
- [ ] 手動在至少一台機器上跑 `dpm upgrade-self`,確認能抓到剛發布的 release 並換裝成功
