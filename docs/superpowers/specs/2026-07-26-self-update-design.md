# dpm 自我更新設計

日期:2026-07-26

## 背景與動機

`dpm` 有三個概念上不同的「更新」,目前只有兩個有真實邏輯:

- `dpm update` — 刷新本地套件索引(從 `setting_config.sources` 重抓每個來源的 `RepoInfo.json`),已存在。
- `dpm upgrade` — 升級已裝套件到解析出的最新版本,已存在(`crates/dpm/src/action.rs::install_resolved`,`2026-07-26` 稍早的重構已經把這個從除錯輸出改成真的會呼叫解析+安裝邏輯)。
- `dpm upgrade-self` — 升級 `dpm` 這個執行檔本身,目前是空殼(`ActionInfo::upgrade_self`,`crates/dpm/src/action.rs:442`,只印一行字,沒有任何邏輯)。

這次要補的是第三個:讓 `upgrade-self` 真的用 `self_update` v0.44.0 crate 檢查 GitHub Release、下載對應平台的二進位檔、原地換裝。

`self_update` 的 GitHub backend 需要 repo 有照平台/架構分開的 release asset 可以抓,但目前 `Derrick-Program/DPM-Workspace` 沒有任何 GitHub Release,也沒有 CI 在編譯發布用的 binary(`.github/workflows/` 現有的 `publish.yml`/`pr-check.yml` 都不做這件事)。所以這次也要把「打 tag 之後編多平台 binary 並發到 GitHub Release」的 workflow 建起來,`upgrade-self` 才有東西可以抓。

另外,自我更新下載回來的執行檔必須驗證是官方簽的,不能單靠「從 GitHub Release 下載」這件事本身當作信任依據(下載通道被劫持/repo 被入侵頂替 release 內容都不是不可能)。這次一併加上簽章驗證:`self_update` 內建的 `signatures` feature(基於 [zipsign](https://github.com/Kijewski/zipsign),ed25519)——公鑰編進 client binary,release CI 用對應私鑰簽每個 artifact,驗證失敗就拒絕安裝並明顯報錯,不是靜默略過。

**範圍澄清**:這次只處理「dpm 這個執行檔本身」的簽章(單一官方金鑰,CI 簽、client 內建公鑰驗)。使用者另外提出的「套件作者身份驗證系統」(`dpm install`/`update` 時驗證*套件*是哪個作者發布的,多作者、PR 需附公鑰、`dpm-server`/`dpm-core` schema 要加簽章欄位)是完全不同、範圍大很多的系統,不在這份 spec 裡,另外開新的 brainstorming/spec 處理。

## 目標

- `ActionInfo::upgrade_self()` 改成用 `self_update` v0.44.0 crate 做真的版本檢查 + 下載 + 簽章驗證 + 換裝。
- 新增 `.github/workflows/release.yml`:push `v*.*.*` tag 時觸發,編 4 個平台(macOS x86_64/aarch64、Linux x86_64/aarch64)的 `dpm` binary,用 zipsign 簽章,打包成 `self_update` 認得的 target-triple 命名格式,發到 GitHub Release。
- `crates/dpm/Cargo.toml` 的 `git2` 加上 `vendored-openssl`/`vendored-libgit2` feature,讓跨平台編譯不依賴系統 libgit2/openssl。
- 一次性產生 zipsign 簽章金鑰對,公鑰(32 bytes)commit 進 repo 並編進 client binary,私鑰(64 bytes)存進 GitHub Actions secret。

## 非目標

- 不處理 `dpm` 執行檔本身的權限自動提升。目前 `dpm` 是透過 `cargo install --path crates/dpm`(裝進 `~/.cargo/bin`)取得,使用者説明未來會有安裝/移除腳本把 `dpm` 放進系統 PATH(如 `/usr/local/bin`)、擁有者是 root——那種情況下升級需要寫權限,做法是使用者自己 `sudo dpm upgrade-self`,不是 dpm 自動偵測 `--system` flag 或自動 re-exec 提權。`--system` flag 本身只控制*套件*安裝位置(`ctx.install_dir`/`ctx.bin_dir`),跟 `dpm` 自己的執行檔位置無關,兩者不要混在一起處理。
- 不做「安裝/移除腳本」本身(使用者提到未來會做,但這次範圍只到 `upgrade-self` 本身能不能正常運作)。
- 不做套件作者身份驗證系統(上面「範圍澄清」提到的那個)——`dpm install`/`dpm update` 抓的套件目前沒有簽章驗證,維持現狀,另開 spec 處理。
- 不用 GPG。zipsign(ed25519)是 `self_update` crate 原生支援的簽章機制,client 端不需要額外裝 `gpg` 指令或管理 keyring,信任模型(公鑰編進 binary、簽章隨檔案走、驗證失敗拒絕安裝)跟 GPG 簽章目標一致,但零外部依賴、零 keyring 管理。
- 不改動 `dpm update`/`dpm upgrade` 既有邏輯。

## 架構

### 簽章金鑰(zipsign,一次性 bootstrap)

不在 CI 裡自動產生——簽章金鑰是敏感的一次性動作,由人手動執行:

```
cargo install zipsign --locked
zipsign gen-key dpm-release-signing.priv dpm-release-signing.pub
```

產出:

- `dpm-release-signing.priv`:64 bytes 原始二進位(ed25519 keypair,`SigningKey::to_keypair_bytes()`)。**不 commit**,轉 base64 後存進 GitHub repo 的 Actions secret `ZIPSIGN_SIGNING_KEY_B64`,本機留存的檔案自行妥善保管或刪除。副檔名刻意用 `.priv` 而不是 `.key`——`.gitignore` 已經在前面 Quick Win 批次加過 `*.key`,但那條規則是防範一般用途的金鑰檔意外入庫,不能保證涵蓋這裡刻意挑的檔名,用 `.priv` 明確跟 `pub` 檔對應、一眼看出是哪一半,不依賴 gitignore pattern 是否剛好匹配。
- `dpm-release-signing.pub`:32 bytes 原始二進位(`VerifyingKey::as_bytes()`)。**commit 進 repo**——這是公鑰,公開沒有風險,commit 到 `crates/dpm/keys/dpm-release-signing.pub`。

保險起見,`.gitignore` 另外明確加一條 `dpm-release-signing.priv`,即使有人不小心在 repo 根目錄手滑產生私鑰檔也會被擋。

### `upgrade_self()` 實作

```rust
const RELEASE_SIGNING_PUBLIC_KEY: &[u8; 32] = include_bytes!("../keys/dpm-release-signing.pub");

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
        .map_err(|e| ClientError::SystemError(format!("failed to configure self-update: {e}")))?
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

- `verifying_keys([*RELEASE_SIGNING_PUBLIC_KEY])` 是唯一新增的一行設定——簽章驗證本身完全是 `self_update`/`zipsign_api` 內部做的,沒有自己刻驗證邏輯。驗證失敗(缺簽章、簽章對不上這把公鑰)`self_update` 會直接回傳 `Err`,**不會**下載完還繼續裝——`update()` 這一步本身就是「驗證通過才換裝,沒通過整個操作直接失敗」,不是「先裝再補驗證」。
- `is_signature_error` 檢查 `self_update::errors::Error::NoSignatures` / `Error::Signature`(兩個都是簽章相關的錯誤 variant)。這條分支印出來的訊息故意跟其他錯誤分開、標成 `INSECURE:`——這是使用者原本要的「明顯顯示有問題」,不是跟一般網路錯誤混在同一句話裡。
- `is_permission_denied` 是另一個小 helper,檢查 `self_update::errors::Error::Io` 底下的 `io::ErrorKind::PermissionDenied`。
- 回傳型別從 `()` 改成 `ClientResult<()>`——目前 `upgrade_self` 簽名是 `pub fn upgrade_self(&self)`(見 `crates/dpm/src/lib.rs:88` 呼叫端 `ActionInfo::new(...).upgrade_self()`,沒有 `?`),這次一起改成回傳 `Result`,呼叫端補上 `?`,理由:`self_update` 的每一步(查詢 GitHub API、下載、驗簽、解壓、換裝)都可能失敗,原本的空殼簽名沒這個問題所以看不出來,現在是時候讓錯誤能傳出去而不是 `.unwrap()` 或吞掉。
- `bin_name`/`repo_owner`/`repo_name` 三個值是編譯期常數,直接寫死在函式裡(不透過 config 檔——這是 dpm 更新自己專用的邏輯,不是使用者可設定的套件來源,沒有理由讓它可設定)。
- `current_version(env!("CARGO_PKG_VERSION"))` 讀的是編進 binary 的版本號,跟根 `Cargo.toml` 的 `[workspace.package] version` 是同一個值(`version.workspace = true` 繼承鏈)。

### `Cargo.toml` 改動

`crates/dpm/Cargo.toml`:

```toml
self_update = { version = "0.44.0", features = ["archive-tar", "compression-flate2", "signatures"] }
git2 = { version = "0.18.1", features = ["vendored-openssl", "vendored-libgit2"] }
```

`self_update` 的 features 視 release 產物打包格式而定——目前規劃用 `.tar.gz`(macOS/Linux 都用同一種,不用另外處理 `.zip`),所以開 `archive-tar`(tar 格式)+ `compression-flate2`(gzip 解壓縮)+ `signatures`(zipsign 簽章驗證)。*(初版 spec 誤寫成 `compress`,已依 docs.rs 上 0.44.0 的實際 feature 名稱修正。)*

### Release workflow(`.github/workflows/release.yml`)

觸發條件:`on: push: tags: ["v*.*.*"]`——跟現有 `just tag-release`(讀 `Cargo.toml` 版本、本地打 annotated tag,不自動 push)的既有慣例銜接,使用者流程不變:`just tag-release` → 確認 → `git push origin vX.Y.Z` → 這個 workflow 自動接手。

Job matrix(4 組):

| target                        | runner                             |
| ----------------------------- | ---------------------------------- |
| `x86_64-apple-darwin`       | `macos-latest`                   |
| `aarch64-apple-darwin`      | `macos-latest`                   |
| `x86_64-unknown-linux-gnu`  | `ubuntu-latest`                  |
| `aarch64-unknown-linux-gnu` | `ubuntu-latest`(cross toolchain) |

每個 target 的 job 步驟:

1. 用 [`taiki-e/upload-rust-binary-action`](https://github.com/taiki-e/upload-rust-binary-action)(`dry-run: true`)處理 cross-compile + 打包成 `dpm-<target>.tar.gz`,但**不**讓它直接上傳——`dry-run: true` 只編譯+打包,不碰 Release,這樣才有機會在上傳前插入簽章這一步。輸出用 action 的 `tar` output 拿到產生的檔名。
2. `cargo install zipsign --locked`(zipsign 沒有發預編譯 binary,CI 裡現場編;只有 tag push 才跑這個 workflow,頻率低,編譯時間可接受)。
3. 把 `ZIPSIGN_SIGNING_KEY_B64` secret base64 decode 回私鑰檔(`echo "$SECRET" | base64 -d > dpm-release-signing.priv`)。
4. `zipsign sign tar dpm-<target>.tar.gz dpm-release-signing.priv`(原地簽,簽章直接附加在 `.tar.gz` 裡,不產生額外檔案)。
5. `rm dpm-release-signing.priv`(簽完立刻刪,不留在 runner 上)。
6. `gh release upload "$TAG" "dpm-<target>.tar.gz"` 手動上傳已簽章的檔案(取代 action 原本內建的上傳,因為簽章要插在打包完、上傳前這個空隙)。

每個 target 產出 `dpm-<target>.tar.gz`,內含 `dpm` 執行檔——`self_update` 預設用 `self_update::get_target()`(回傳目前執行環境的 Rust target triple字串)比對 release asset 檔名裡有沒有包含該字串,所以檔名裡的 target triple 一定要跟 Rust 官方 target 名稱完全一致,不能自己另外發明命名規則。

Workflow 只建 `dpm`(client)的 release,不含 `dpm-server`——`dpm-server` 目前沒有自我更新需求,不在這次範圍內。

### 資料流

```
使用者執行 dpm upgrade-self
  -> self_update 查 GitHub API:GET /repos/Derrick-Program/DPM-Workspace/releases/latest
  -> 比對 release tag(vX.Y.Z)與 current_version
  -> 相同 或 target 找不到對應 asset -> 回報並結束(Status::UpToDate 或 Err)
  -> 版本較新 -> 下載對應 target 的 dpm-<target>.tar.gz 到系統暫存目錄
  -> 用編進 binary 的公鑰驗證 zipsign 簽章
       -> 驗證失敗 -> Err(NoSignatures/Signature),不解壓、不換裝,回報 INSECURE
  -> 驗證通過 -> 解壓、取出 dpm 執行檔
  -> 原地覆蓋 current_exe()(self_update 內部用暫存檔 + rename,同檔案系統的原子替換)
  -> 回報 Status::Updated(新版本號)
```

## 錯誤處理

- GitHub API 查詢失敗(離線、rate limit)→ `self_update` 回傳 `Err`,包成 `ClientError::SystemError`,原樣往上拋,`main.rs` 既有的 `eprintln!` + `exit(1)` 路徑接手,不用新增處理。
- 目標平台沒有對應 release asset(例如某次 release 只成功發了 3 個平台)→ `self_update` 回傳 `Err`(`ReleaseNotFound`/類似錯誤),同上直接往上拋,不特別攔截。
- **簽章驗證失敗**(缺簽章、簽章對不上公鑰、release 內容被竄改)→ 攔截 `Error::NoSignatures`/`Error::Signature`,印出獨立的 `INSECURE:` 標記訊息,明確拒絕安裝(不解壓、不換裝、不繼續往下走任何一步)。這是這次新增的、唯一需要跟其他錯誤分開處理的分支——其餘錯誤都是「查不到/連不上」這種可重試的狀況,簽章失敗是「這個檔案不可信」,語意不同,訊息也要讓使用者一眼看出差異。
- 寫入權限不足(執行檔所在目錄不可寫)→ 攔截 `io::ErrorKind::PermissionDenied`,額外印一行 `sudo dpm upgrade-self` 提示,不自動提權。

## 測試計畫

- `self_update` crate 本身的 GitHub API 互動不寫整合測試(需要真實網路 + 真實 GitHub release,跟 `crates/dpm-core/tests/test.rs::test_from_url` 那種打真網路的測試是同一類問題,這次不重蹈覆轍)。
- `is_permission_denied` helper 寫一個小的 `#[test]` 直接建構 `io::Error::new(io::ErrorKind::PermissionDenied, "x")` 包進 `self_update::errors::Error::Io`,驗證回傳 `true`;其他 `ErrorKind`(如 `NotFound`)驗證回傳 `false`。
- `is_signature_error` 同樣寫 `#[test]`,涵蓋 `Error::NoSignatures`/`Error::Signature` 兩個 variant 回傳 `true`,其他 variant(如 `Error::Network`)回傳 `false`。
- `release.yml` workflow 本身用「打一個測試 tag 觸發一次真實 run,人工確認 4 個 asset 都出現在 GitHub Release 頁面、且用本機 `zipsign verify tar dpm-<target>.tar.gz pub.key` 確認簽章有效」驗證,不寫自動化測試(CI workflow 正確性本來就是靠跑一次來驗證,寫 meta-test 意義不大)。
- 手動驗證「簽章驗證真的會擋下未簽章/竄改過的檔案」:本機把 `verifying_keys` 指到一把跟 CI 簽章不同的假金鑰,重跑一次 `dpm upgrade-self`,確認回報 `INSECURE:` 而不是靜默裝上去——這是這次新增行為裡最重要的一條路徑,一定要實際跑過,不能只看程式碼邏輯合理就當作沒問題。

## 驗證清單

- [ ] `cargo check --workspace` 通過
- [ ] `cargo clippy --workspace --all-targets` 通過
- [ ] `cargo test --workspace` 通過(含新增的 `is_permission_denied`/`is_signature_error` 測試)
- [ ] 一次性執行 `zipsign gen-key`,`dpm-release-signing.pub` commit 進 `crates/dpm/keys/`,`dpm-release-signing.priv` base64 存進 GitHub secret `ZIPSIGN_SIGNING_KEY_B64`,確認 `.gitignore` 有擋到 `dpm-release-signing.priv`
- [ ] 打一個真實 tag、push、觀察 `release.yml` 跑完,GitHub Release 頁面出現 4 個已簽章的 `dpm-<target>.tar.gz` asset
- [ ] 手動在至少一台機器上跑 `dpm upgrade-self`,確認能抓到剛發布的 release、簽章驗證通過並換裝成功
- [ ] 手動驗證簽章不符時會被擋下(見上方測試計畫最後一條)
