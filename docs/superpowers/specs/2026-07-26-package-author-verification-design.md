# 套件作者身份驗證設計

日期:2026-07-26

## 背景與動機

`dpm-server`/`dpm` 目前對「官方來源」(`OFFICIAL_REPO_URL` 指向的 `Derrick-Program/DPM-Server` repo)的套件只做**完整性**驗證——`fetch_and_verify_prebuilt` 比對下載回來的檔案 blake3 hash 跟 `RepoInfo.json` 記錄的 hash 是否一致,`kind: source` 的套件則完全沒有驗證(`build_command` 字串直接在本機執行)。這保護的是「下載過程沒有壞掉/被竄改成別的內容」,但不保護「這個 hash 本身是不是本來就該屬於這個套件」——只要有人能把一個新的 `(hash, url)` 寫進 `RepoInfo.json`(不管是拿到 repo 寫入權限、CI 被入侵、還是惡意 PR 沒被注意到),`hash` 驗證照樣會通過,因為驗證的東西本身就是攻擊者換掉的。

這次要補的是「這個套件版本真的是它宣稱的作者發布的」這一層,防的是套件被接手/冒名頂替後推出惡意新版本(supply-chain hijack)。跟 [`2026-07-26-self-update-design.md`](2026-07-26-self-update-design.md)(dpm 執行檔本身的簽章)是兩個獨立系統:那份用單一固定金鑰保護 dpm 二進位檔本身;這份要處理的是多個(會持續增加的)套件作者,金鑰不能編進 client binary,必須即時抓。

**範圍**:只處理透過 `source.repo_url == OFFICIAL_REPO_URL` 這個官方來源安裝的套件。使用者自行加的第三方 source 維持現狀(沒有簽章驗證,使用者自行承擔風險,`install_source_package` 既有的「not vetted by the DPM team」警告已經在提示這件事)。

## 威脅模型與已知限制

**擋得住**:套件某個版本的簽章跟這個套件名稱之前登記的作者對不上——不管是攻擊者拿到 `RepoInfo.json` 的寫入權限直接改內容、還是一個惡意 PR 想冒充既有套件的作者發新版本,只要簽章驗不過,`dpm-server fix add`(發布端)或 `dpm` 的 client 驗證(安裝端)都會拒絕。

**擋不住**(明確告知使用者,不誇大保護範圍):作者公鑰本身(`keys/<author_id>.pub`)是 client 即時從官方 repo 抓的,信任層級跟 `RepoInfo.json` 本身一樣——如果官方 repo 整個被接手(不只是改 `RepoInfo.json`,連 `keys/` 目錄也能改),攻擊者可以連公鑰一起換掉,形式上還是驗證得過。這個系統防的是「這個版本沒有經過原作者的私鑰簽署」,不是防「官方 repo 完全淪陷」——後者是 branch protection/帳號安全那個層級的問題,不是簽章系統要解的。

**已知、刻意延後的限制**:`kind: source` 套件簽的 hash 是 `build_command + 發布當下的 commit`,但 commit 本身沒有發布到任何 client 端可以驗證的地方——簽章只證明「這串 build 指令是作者發布的」,不證明「client 實際 clone 下來執行的原始碼樹跟簽署當下一致」。目前不是可被利用的漏洞,只是尚未涵蓋的範圍。

## 目標

- `dpm-server` 新增 `keygen`/`sign` 子指令,`init` 加上 `--author` 必填參數。
- `dpm-server fix add` 新增作者一致性檢查:同一套件名稱的後續版本,簽章必須對得上第一次發布時登記的作者,對不上拒絕寫入 `RepoInfo.json`。
- `dpm-core` 的 `PackageVersionInfo` 新增 `author`/`signature` 欄位(`Option`,向下相容)。
- `dpm` 的 `sync_source()`(`update`)跟安裝流程(`install_resolved`)各自對官方來源的套件重新驗證一次簽章,驗不過拒絕(不是靜默略過)。
- `dpm-core` 新增共用的簽章/驗證 primitive(ed25519),`dpm`/`dpm-server` 都呼叫同一份實作。

## 非目標

- 不處理官方 repo 本身被完全接手的情況(見上方「威脅模型與已知限制」)。
- 不做多簽章/threshold 簽章(一個套件名稱一次只認一個作者、一把金鑰)。
- 不做金鑰輪替機制(作者換金鑰目前沒有設計——如果真的需要,先手動處理:舊金鑰的套件維持能裝,新版本换個角度看待成「新作者接手」需要走跟原作者一樣嚴格的人工審核流程,這次不特別設計自動輪替)。
- 不影響第三方(非官方)來源的安裝流程,那邊維持現狀零驗證。
- 不做本次 [dpm 自我更新簽章](2026-07-26-self-update-design.md) 已經涵蓋的東西(那是完全獨立的系統,單一固定金鑰)。

## 架構

### Repo 佈局(`Derrick-Program/DPM-Server`,獨立於 `DPM-Workspace` 的資料 repo)

```
keys/<author_id>.pub   — 32 bytes 原始二進位,ed25519 verifying key
packages/<name>/packageInfo.json  — 新增 author/signature 兩個欄位
```

`author_id` 是作者自選的任意字串(例如 GitHub username),同一個 `author_id` 底下可以發多個套件(作者證件獨立於套件,不是每個套件名稱各自綁一把金鑰)。新作者的第一個 PR 必須連同 `keys/<author_id>.pub` 一起提交——這就是人工審核的信任閘門:maintainer 審這個 PR 時,審的是「這把公鑰真的是這個人的」,不是每次發新版本都要審。

### `dpm-core` 共用 primitive

跟 `hash_file`/`zip_folder` 同等級的共用工具函式,不特別 feature-gate(這不是 `RepoInfo` 的 CRUD,是獨立小工具,`dpm`/`dpm-server` 都需要,`dpm-server` 的測試也需要自我驗證剛簽出來的東西,ungate 最單純):

```rust
// crates/dpm-core/src/lib.rs
pub fn sign_hash(signing_key: &ed25519_dalek::SigningKey, hash_hex: &str) -> String {
    let sig: ed25519_dalek::Signature = signing_key.sign(hash_hex.as_bytes());
    hex::encode(sig.to_bytes())
}

pub fn verify_hash_signature(
    verifying_key: &ed25519_dalek::VerifyingKey,
    hash_hex: &str,
    signature_hex: &str,
) -> CoreResult<()> {
    // hex decode signature_hex -> Signature,verifying_key.verify(hash_hex.as_bytes(), &sig)
    // 任何一步失敗(hex 格式錯、簽章長度不對、驗證不過)都回傳同一種 CoreError::SignatureInvalid
}
```

簽的是 `packageInfo.json` 的 `hash` 欄位(hex 字串本身,不是重新雜湊一次)——`Prebuilt` 套件已經有這個欄位(整個套件目錄的 blake3);`Source` 套件目前沒有,這次一併補上(`dpm-server hash` 對 `kind: source` 的套件也算出一個 hash,內容是 `build_command` 字串 + 來源 repo 的 commit hash 一起雜湊,讓 `Source` 套件從「零驗證」也進到「有東西可以簽、可以驗」的狀態)。

`PackageKind::Source` 因此也要加一個 `hash: Option<String>` 欄位(現有 `PackageKind::Source { build: String }` 只有 `build`,沒有 hash 概念)。

### `dpm-server` CLI 新增/修改

- `dpm-server keygen <author_id>`:產生 `keys/<author_id>.priv`(本機,不 commit,`.gitignore` 擋)+ `keys/<author_id>.pub`(commit 進 repo,PR 提交)。已存在同名金鑰時拒絕覆蓋,除非 `--force`。
- `dpm-server init <name> <entry> --author <author_id> [-v ver] [-d description]`:`--author` 改成必填。`init` 前先檢查 `keys/<author_id>.pub` 存在,不存在直接報錯提示先跑 `keygen`——避免有人 init 到一半才發現沒金鑰。`author_id` 寫進新建的 `packageInfo.json`。
- `dpm-server sign <name>`:讀 `packages/<name>/packageInfo.json` 的 `author`(決定用哪把私鑰)跟 `hash`,呼叫 `dpm_core::sign_hash`,把結果寫回 `packageInfo.json` 的新 `signature` 欄位。必須在 `hash` 之後執行(依賴 `hash` 欄位已經算好),`build_command` 有改過而沒重新 `hash`+`sign` 的話,`fix add` 那邊驗證會直接失敗,自然擋下「忘記重簽」的情況。
- `dpm-server fix add <name> ...`(既有指令擴充):
  1. 讀 `packageInfo.json` 的 `author`/`signature`/`hash`。
  2. 讀 `keys/<author>.pub`,用 `dpm_core::verify_hash_signature` 驗 `signature` 對不對得上 `hash`——驗不過直接拒絕,不管是不是新套件。
  3. 查這個套件名稱在目前 `RepoInfo` 裡已有的版本:沒有 → 這是第一次發布,`author` 直接登記(沒有「跟誰比對」的問題);有 → 新版本的 `author` 必須等於既有版本的 `author`,不等就拒絕(這就是防冒名頂替的核心檢查)。
  4. 通過才把 `author`/`signature` 一起寫進 `RepoInfo.json` 對應的 `PackageVersionInfo`。

### `dpm` client 驗證

**信任閘門用 `repo_url` 判斷,不是 alias 字串**——`source_alias == "official"`(既有程式碼裡用來印警告的判斷)是使用者本機 `config.json` 可以自己編輯的東西,理論上使用者可以自己加一個 alias 也叫 `"official"` 的第三方 source。簽章驗證這種安全性判斷改用 `source.repo_url == OFFICIAL_REPO_URL`(`system.rs` 既有常數)——這是程式寫死的值,使用者本機設定改不了它。

兩個驗證點,共用同一個驗證 helper(縱深防禦,跟這次其他部分的一貫做法一致):

1. **`sync_source()`**(`dpm update` 跟第一次執行的 `init_update` 都會走到):對 `repo_url == OFFICIAL_REPO_URL` 的來源,每個套件版本抓 `keys/<author>.pub`(URL 推導方式比照 `official_repo_info_url()`,同一個 repo 底下的 raw content URL)驗證 `signature`/`hash`。驗不過的**只跳過那一筆**,印出明顯警告(套件名、版本、作者、原因),不中斷整個 `update`——不能因為一個作者的一筆簽章有問題,連帶讓其他作者的合法套件也裝不到。同一次 `sync_source()` 執行期間,同一個 `author_id` 只抓一次公鑰(cache 在記憶體裡,不是每個套件版本各自打一次網路)。
2. **安裝當下**(`install_resolved`/`fetch_and_verify_prebuilt`):從本機 DB 讀出的 `DbPackage`(已經帶 `author`/`signature`/`hash`)在真正下載/執行 build_command 之前**再驗一次**。防的是 `update` 跟 `install` 之間本機 DB 被動過手腳。驗不過直接拒絕安裝,訊息格式跟自我更新那邊的 `INSECURE:` 標記一致,讓使用者一眼看出這是「不可信」而不是普通的網路錯誤。

### 本機 DB schema

`DbPackage` 新增 `author: Option<String>`、`signature: Option<String>` 兩欄,新增 migration `0004_package_signatures`(沿用前面 `entry` 欄位改 nullable 時建立的「整表 DROP+CREATE」慣例),`COLUMNS`/`row_to_package`/`insert` 三處同步更新(參照既有 `entry` 欄位改動時的模式)。

### 資料流(`dpm update` 為例)

```
dpm update
  -> sync_source() 對每個 configured source:
       -> fetch RepoInfo.json
       -> repo_url == OFFICIAL_REPO_URL?
            是 -> 對每個 PackageVersionInfo:
                    -> author 有登記記錄的公鑰快取嗎?沒有就抓 keys/<author>.pub
                    -> verify_hash_signature(pub_key, hash, signature)
                         -> 失敗 -> 印警告,跳過這筆,不寫進本機 DB
                         -> 成功 -> 正常寫進本機 DB(含 author/signature)
            否 -> 照舊,不驗證,全部寫進本機 DB

dpm install <pkg>
  -> install_resolved() 解析出要裝的 (source, name, version)
  -> 從本機 DB 讀出 DbPackage(含 author/signature/hash)
  -> source repo_url == OFFICIAL_REPO_URL?
       是 -> 再驗一次簽章 -> 失敗 -> INSECURE,拒裝
       否 -> 照舊
  -> 驗證通過(或不需要驗證)-> 正常走下載/build 流程
```

## 錯誤處理

- `sync_source()` 遇到簽章驗證失敗:警告等級輸出(套件名+版本+作者+失敗原因),**跳過該筆**,`update` 整體視為成功結束(部分套件被跳過不算 `update` 失敗)。
- 安裝時遇到簽章驗證失敗:`ClientError`,訊息帶 `INSECURE:` 標記,整個 `install`/`upgrade` 該筆操作失敗,不部分安裝。
- `dpm-server fix add` 遇到作者不符/簽章驗證失敗:`ServerError::ValidationError`,明確說明是「作者不符」還是「簽章驗證失敗」兩種不同原因(訊息不要含糊,讓發布者知道是要重新 `sign` 還是真的用錯金鑰)。
- 抓 `keys/<author>.pub` 本身失敗(網路問題、檔案不存在——例如 `RepoInfo.json` 記錄了某個 `author_id` 但 `keys/` 目錄下沒有對應檔案,資料不一致):視同驗證失敗處理,不是另外開一種「查無此人」的寬容模式。

## 測試計畫

- `dpm_core::sign_hash`/`verify_hash_signature`:單元測試,正常簽驗、簽章對不上的 hash、竄改過的 signature 字串、非法 hex 輸入。
- `dpm-server`:`keygen` 產生的金鑰檔案大小/格式正確;`init --author` 在金鑰不存在時拒絕;`fix add` 的作者一致性檢查——同套件第二版用不同作者的金鑰簽,驗證應該拒絕;同作者正常應該通過。
- `dpm` client:`sync_source()` 用本地 fixture(比照 `fetcher.rs::serve_once` 的 TCP mock 手法,不打真網路)模擬一個簽章正確、一個簽章錯誤的版本,驗證只有正確的那筆進到本機 DB;安裝路徑用類似手法驗證 INSECURE 拒裝路徑真的會擋下。

## 驗證清單

- [ ] `cargo check --workspace` / `cargo clippy --workspace --all-targets` / `cargo test --workspace` 通過
- [ ] `dpm-server keygen`/`init --author`/`sign`/`fix add` 走一輪完整流程,產出的 `RepoInfo.json` 含 `author`/`signature`
- [ ] 手動測試:同套件用不同作者金鑰嘗試 `fix add` 第二版,確認被拒絕
- [ ] 手動測試:`dpm update` 對著一個混合正確/錯誤簽章的測試 `RepoInfo.json` 跑,確認錯誤簽章的版本被跳過、正確的照常進本機 DB,且 `update` 本身不報失敗
- [ ] 手動測試:本機竄改 DB 裡某筆 `signature`,`dpm install` 該套件確認被 INSECURE 拒裝
