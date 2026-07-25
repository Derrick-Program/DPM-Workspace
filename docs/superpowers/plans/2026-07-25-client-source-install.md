# Client 端 Source 套件安裝(Phase 4) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 讓 `dpm install <pkg>` 支援 `kind: "source"` 的套件——client 把套件來源 repo 的 `packages/<pkg>/` 淺層 clone 下來,在 staging 目錄裡以呼叫者當下的權限(絕不因為 `--system` 就用 root)執行 `packageInfo.json` 裡記錄的 `build` 指令,`$OUT` 環境變數指向這次要生產的安裝內容,build 成功後透過既有的 `swap_into_install_dir` 原子換裝進最終安裝路徑。同時補上 `dpm-server` 這邊發布 Source 套件的能力(`fix add --build`),不然 client 端永遠測不到真的 Source 套件。這是 `docs/superpowers/specs/2026-07-24-multi-source-registry-design.md` Section 8-9 的實作。

**Architecture:** 五個循序 task。Task 1 補 `dpm-server` 發布 Source 套件的能力(沒有這個,Task 2-4 沒有真實資料可以測)。Task 2 修正 `Source.repo_url` 的既有語意問題(目前是「給人看」的 GitHub 網頁 URL,不是真的 git 可 clone 的 remote)並啟用 `git2` 依賴。Task 3 寫一個獨立、可測試的 clone 輔助模組(淺層 clone + 取出 `packages/<pkg>/` 子目錄,不做真的 sparse-checkout——見下方差異點)。Task 4 是核心整合:`install()` 依 `kind` 分岔,新增 Source 分支。Task 5 收尾驗證。

**Tech Stack:** Rust 2021、`git2 = "0.18.1"`(已經是 `dpm` 的既有依賴,目前完全沒用到——這次真正啟用它,同時解決 TODO.md 已經記錄的「死依賴」項目)。

## 與 spec 的差異(刻意的範圍收斂,附理由)

1. **不做真的 sparse-checkout,改成「淺層(depth=1)clone 整個 repo,只取用 `packages/<pkg>/` 子目錄」。** spec 自己在「非目標」段就承認「`source` 模式套件原始碼的確切傳輸機制...列為架構決策,實作時再定案」——這裡的決策是:`git2`(綁定 libgit2)的 sparse-checkout 支援不如 git CLI 本身成熟穩定,而 spec 說的「小文字檔,PR 好審」代表這些 repo 本來就不大,犧牲一點頻寬換取實作簡單、用得到的 API 直接可靠,是合理的取捨。之後如果真的遇到大型來源 repo 造成 clone 太慢,再回頭補真的 sparse-checkout(用 `git2::Repository` 的 sparse-checkout API,或乾脆 shell out 到 `git` CLI 本身)。
2. **`Source.repo_url` 的語意重新定義:它現在必須是「git 可直接 clone 的 remote URL」,不能再是給人看的網頁連結。** 目前 `system.rs::init()` 塞給預設 `official` 來源的 `repo_url` 是 `https://github.com/Derrick-Program/DPM-Server/tree/main/Repo`——這是 GitHub 網頁的 tree view 路徑,不是 `git clone` 可以直接吃的 URL。這個欄位在 Phase 2/3 從來沒有被拿去 clone 過(只有 `repo_info` 被拿去打 JSON API),所以這個問題到現在才第一次真正被踩到。修正成 `https://github.com/Derrick-Program/DPM-Server`(拿掉 `/tree/main/Repo`)。第三方來源透過 `dpm source add <url>` 加入時本來就是把使用者輸入的同一個字串同時塞進 `repo_url`/`repo_info` 兩個欄位——只要使用者輸入的是一個真的 repo URL(而不是某個網頁子路徑),這條路徑不用改就能動,不需要新增 schema 欄位。
3. **建置(`build` 指令)執行完全不經過 `system_command_runner`。** 這不是新設計的隔離機制,而是這個 codebase 現有行為的自然結果:`dpm` 整個程式從來沒有在任何地方對「自己這個行程本身」做權限提升(`sudo` crate 有裝但從沒被呼叫過,唯一的提權手法是 `system_command_runner` 內部把外部指令字串加上 `sudo` 前綴這件事本身)。只要 build 指令用一般的 `std::process::Command` 直接執行、不透過 `system_command_runner`,它天生就是用呼叫者當下的權限跑,完全符合 spec「build 指令永遠用當前呼叫者權限跑」的要求,不需要額外寫任何隔離邏輯。
4. **原子換裝(`swap_into_install_dir`)沿用既有實作,不修正它在 `--system` scope 下可能的權限落差。** 現有的 `swap_into_install_dir` 對 Prebuilt 套件本來就是用純 `std::fs::rename`,完全不經過任何提權機制——`--system` scope 下如果 `/opt/...` 已經被 `permision_check()` chown 成 root:root(Linux)/`<user>:admin`(macOS),這個 rename 有沒有辦法成功,取決於整個 `dpm` 行程當下是不是已經用 root 執行(這件事這次沒有去查證,也不是這個 phase 的範圍)。這是 TODO.md 已經記錄的「權限模型不一致」既有問題,Source 套件透過同一個函式換裝,繼承一模一樣的既有行為——不會比 Prebuilt 更糟,但也不會比它更好。修好整個 `--system` 權限模型是獨立的既有 TODO 項目,不在這個 plan 動。

## Global Constraints

- 這個 phase 不新增 `semver`/`pubgrub` 依賴。
- Build 指令的執行**絕對不能**經過 `SystemController::system_command_runner`(那個函式在 `--system` scope 下會把指令加上 `sudo` 前綴——build 指令不管什麼 scope 都不可以被提權)。
- 非官方來源(`alias != "official"`)的 Source 套件,安裝前要印警告,呼應 `source add` 已經有的「third-party source, not vetted by the DPM team」警告字眼(風格一致)。
- 每個 task 完成後執行 `cargo build --workspace` 確認整個 workspace 仍能編譯;有新增/修改測試的 task 額外跑 `cargo test --workspace`。
- Prebuilt 套件的既有安裝流程(zip 下載、hash 驗證、`swap_into_install_dir`)完全不動。

---

## Task 1: `dpm-server` 支援發布 Source 套件(`fix add --build`)

**Files:**
- Modify: `crates/dpm-server/src/cli_parse.rs`
- Modify: `crates/dpm-server/src/action.rs`

**Interfaces:**
- Consumes:`dpm_core::PackageKind::Source { build: String }`(Phase 2 已存在,`fix_add` 目前完全沒用到它,只建構 `Prebuilt`)。
- Produces:`Add` clap struct的 `url` 欄位改成 `Option<String>`,新增 `build: Option<String>`——兩者互斥,恰好一個要有值。

- [ ] **Step 1: 改 `Add` clap struct**

編輯 `crates/dpm-server/src/cli_parse.rs`,把現有的 `Add` struct(`url: String` 必填、`file_name: Option<String>`)換成:

```rust
#[derive(Args, Debug)]
pub struct Add {
    /// Project Name
    pub project_name: String,
    /// External URL hosting the prebuilt package archive (mutually exclusive
    /// with --build). dpm-server downloads it once to compute its blake3
    /// hash — it does not keep a copy. Must be https://.
    #[arg(long, conflicts_with = "build")]
    pub url: Option<String>,
    /// Override the file name recorded in RepoInfo.json (only meaningful
    /// with --url; defaults to the URL's last path segment)
    #[arg(long)]
    pub file_name: Option<String>,
    /// Shell command clients run locally to build this package from source
    /// (mutually exclusive with --url). $OUT will point at the install
    /// destination when clients actually run it (Phase 4 client-side work).
    #[arg(long, conflicts_with = "url")]
    pub build: Option<String>,
}
```

(`conflicts_with` 讓 clap 自己擋掉「兩個都給」的情況,不用手動檢查那一半;「兩個都沒給」還是要手動檢查,見下一步。)

- [ ] **Step 2: `fix_add` 依 `--url`/`--build` 分岔**

編輯 `crates/dpm-server/src/action.rs`,把 `fix_add`(Phase 3 已經改成下載外部 URL 那版)開頭讀完 `pk_info` 之後、組 `PackageVersionInfo` 之前的部分,改成:

```rust
    let kind = match (&obj.url, &obj.build) {
        (Some(url), None) => {
            if !url.starts_with("https://") {
                return Err(anyhow::anyhow!(
                    "\n--url {} {}",
                    url.yellow(),
                    "must use https://".red()
                ));
            }
            let file_name = obj
                .file_name
                .clone()
                .or_else(|| url.rsplit('/').next().map(|s| s.to_string()))
                .filter(|s| !s.is_empty())
                .ok_or_else(|| {
                    anyhow::anyhow!(
                        "could not derive a file name from --url; pass --file-name explicitly"
                    )
                })?;

            let response = reqwest::blocking::get(url)?;
            if !response.status().is_success() {
                return Err(anyhow::anyhow!(
                    "\nfailed to fetch {}: HTTP {}",
                    url.yellow(),
                    response.status()
                ));
            }
            let bytes = response.bytes()?;
            let tmp_path = std::env::temp_dir().join(&file_name);
            std::fs::write(&tmp_path, &bytes)?;
            let hash = dpm_core::hash_file(&tmp_path)?;
            std::fs::remove_file(&tmp_path)?;

            PackageKind::Prebuilt {
                url: url.clone(),
                hash,
                file_name,
            }
        }
        (None, Some(build)) => PackageKind::Source {
            build: build.clone(),
        },
        (Some(_), Some(_)) => unreachable!("clap's conflicts_with already rejects this"),
        (None, None) => {
            return Err(anyhow::anyhow!(
                "\nfix add {} needs exactly one of {} or {}",
                obj.project_name.yellow(),
                "--url".green(),
                "--build".green()
            ));
        }
    };

    let version_info = PackageVersionInfo {
        version: pk_info.version.clone(),
        kind,
        dependencies: pk_info.dependencies,
        entry: None,
        description: Some(pk_info.description),
    };
    repo.add_package_version(obj.project_name.clone(), version_info)?;
    Ok(())
```

(這段取代 Phase 3 已經寫好的、只處理 `--url` 的整個中段邏輯——URL 分支的內容邏輯完全不變,只是包進 `match` 的第一個分支裡。)

- [ ] **Step 3: 手動驗證兩種模式都能發布(沒有自動測試涵蓋這條路徑,`dpm-server` 沒有 `tests/` 目錄)**

```bash
TMPDIR=$(mktemp -d)
cd "$TMPDIR"
BIN="/Users/derrick/Documents/Program/rust/Project/DPM-Workspace/target/debug/dpm-server"
cargo build --manifest-path /Users/derrick/Documents/Program/rust/Project/DPM-Workspace/Cargo.toml -p DPM-Server

"$BIN" init source-demo main.py -v 1.0.0 -d "a source-kind demo"
"$BIN" fix add source-demo --build "python3 -m py_compile main.py && cp main.py \$OUT/"
cat RepoInfo.json

"$BIN" init prebuilt-demo main.py -v 1.0.0 -d "a prebuilt demo, still works"
"$BIN" fix add prebuilt-demo --url https://raw.githubusercontent.com/rust-lang/rust/master/README.md
cat RepoInfo.json
```

Expected:`RepoInfo.json` 裡 `source-demo` 的條目是 `"kind": "source"`、`"build": "python3 -m py_compile main.py && cp main.py $OUT/"`,沒有 `url`/`hash`/`file_name` 欄位;`prebuilt-demo` 條目維持原本 `"kind": "prebuilt"` 那套欄位——證明兩條路徑互不干擾。跑完清掉 `$TMPDIR`。

- [ ] **Step 4: 整個 workspace 編譯確認**

Run: `cargo build --workspace 2>&1 | tail -40`
Expected: 無錯誤。

- [ ] **Step 5: Commit**

```bash
git add crates/dpm-server/src/cli_parse.rs crates/dpm-server/src/action.rs
git commit -m "$(cat <<'EOF'
feat(dpm-server): fix add --build publishes source-kind packages

Add's --url and --build are now mutually exclusive (clap
conflicts_with) — exactly one is required. --url keeps Phase 3's
external-download-and-hash behavior for PackageKind::Prebuilt;
--build constructs PackageKind::Source { build } directly, no
download/hash step since there's no binary to fetch.

This was deliberately deferred out of Phase 3 (see that plan's own
"與 spec 的差異" section) — client-side Source-kind installs land in
this same plan's later tasks, so publishing and installing ship
together instead of publish support sitting unused.

Co-Authored-By: Claude Sonnet 5 <noreply@anthropic.com>
EOF
)"
```

---

## Task 2: 修正 `Source.repo_url` 語意,啟用 `git2`

**Files:**
- Modify: `crates/dpm/src/lib.rs`
- Modify: `crates/dpm/src/utils/system.rs`
- Modify: `crates/dpm/Cargo.toml`

**Interfaces:**
- Consumes:無。
- Produces:無新型別——`Source.repo_url` 欄位型別不變(`String`),只是文件/語意跟預設值改變:從「給人看的網頁連結」變成「git 可直接 clone 的 remote URL」。

- [ ] **Step 1: 幫 `Source` struct 補文件說明新語意**

編輯 `crates/dpm/src/lib.rs`,把:

```rust
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Source {
    pub alias: String,
    pub repo_url: String,
    pub repo_info: String,
}
```

改成:

```rust
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Source {
    pub alias: String,
    /// Git-clonable remote URL (e.g. `https://github.com/owner/repo`) — this
    /// source's own repo, where `packages/<pkg>/` lives for source-kind
    /// installs. Must be a real clone target, not a human-facing web page.
    pub repo_url: String,
    pub repo_info: String,
}
```

- [ ] **Step 2: 修正預設 `official` 來源的 `repo_url`**

編輯 `crates/dpm/src/utils/system.rs` 的 `init()`,把:

```rust
                sources: vec![Source {
                    alias: "official".to_string(),
                    repo_url: "https://github.com/Derrick-Program/DPM-Server/tree/main/Repo"
                        .to_string(),
                    repo_info:
```

的 `repo_url` 那行改成:

```rust
                sources: vec![Source {
                    alias: "official".to_string(),
                    repo_url: "https://github.com/Derrick-Program/DPM-Server".to_string(),
                    repo_info:
```

(拿掉 `/tree/main/Repo` 這段網頁路徑——`https://github.com/Derrick-Program/DPM-Server` 本身就是 `git clone` 可以直接吃的 URL。`repo_info` 那行不動。)

- [ ] **Step 3: `git2` 依賴確認/啟用**

編輯 `crates/dpm/Cargo.toml`,確認 `git2 = "0.18.1"` 那行存在(它已經在依賴列表裡,目前完全沒被使用到,這步驟只是確認版本沒問題,不用改動這行本身——Task 3 才會真的寫 `use git2::...`)。

Run: `cargo build -p DPM 2>&1 | tail -20`
Expected: 無錯誤(`git2` 本來就是既有依賴,這步驟不改變依賴關係本身,只是先確認它能正常編譯連結——libgit2 的系統依賴如 `libssl`/`zlib` 如果環境缺少會在這步驟就爆出來,及早發現)。

- [ ] **Step 4: 手動驗證新的 seed 值(沒有自動測試涵蓋 `init()` 的實際檔案系統行為,原因跟 Phase 2 Task 3 的 `config_tests.rs` 說明一樣——`CONFIG`/`MAIN_DIR` 是行程全域的 `OnceLock`,沒有可注入的測試 seam)**

Run:

```bash
TMPDIR=$(mktemp -d)
HOME="$TMPDIR" cargo run --manifest-path /Users/derrick/Documents/Program/rust/Project/DPM-Workspace/Cargo.toml -p DPM -- search anything 2>&1 | tail -5
find "$TMPDIR" -name config.json -exec cat {} \;
```

Expected:找到的 `config.json` 裡 `official` 來源的 `"repo_url"` 是 `"https://github.com/Derrick-Program/DPM-Server"`,沒有 `/tree/main/Repo` 後綴。跑完清掉 `$TMPDIR`。

(如果這個指令組合因為 `directories` crate 在這個環境解析 `HOME` 的方式跟預期不同而找不到 config.json,改成直接讀 `crates/dpm/src/utils/system.rs` 的原始碼確認這行文字改對了即可,不要花時間跟 `directories`/`ProjectDirs` 的路徑解析細節搏鬥——那不是這個 task 的範圍。)

- [ ] **Step 5: Commit**

```bash
git add crates/dpm/src/lib.rs crates/dpm/src/utils/system.rs
git commit -m "$(cat <<'EOF'
fix(dpm): Source.repo_url must be a git-clonable remote, not a web page

repo_url was never actually consumed by any code until this plan's
later tasks (git-cloning a source for source-kind installs) — only
repo_info (the RepoInfo.json index URL) was ever fetched. The seeded
"official" source's repo_url was a GitHub tree-view web path
(.../tree/main/Repo), which git clone can't use directly. Fixed to
the bare repo URL and documented the field's real contract.

Third-party sources added via `dpm source add <url>` already work
under this contract as long as users pass an actual repo URL — no
schema change needed, this only fixes the one hardcoded default.

Co-Authored-By: Claude Sonnet 5 <noreply@anthropic.com>
EOF
)"
```

---

## Task 3: git clone 輔助模組(淺層 clone + 取出套件子目錄)

**Files:**
- Create: `crates/dpm/src/utils/source_clone.rs`
- Modify: `crates/dpm/src/utils/mod.rs`

**Interfaces:**
- Consumes:`git2`(Task 2 已確認可用)。
- Produces:`pub fn clone_package_source(repo_url: &str, package_name: &str, clone_into: &Path) -> ClientResult<PathBuf>`——把 `repo_url` 淺層 clone 進 `clone_into`,回傳 clone 出來的 tree 裡 `packages/<package_name>/` 的絕對路徑;找不到該子目錄就回傳 `CoreError::PackageNotFound`。

- [ ] **Step 1: 寫失敗的測試(先紅)——對一個本機 git repo clone**

建立 `crates/dpm/src/utils/source_clone.rs`,先只放測試模組(函式先留 `unimplemented!()`):

```rust
use crate::{ClientError, ClientResult};
use dpm_core::CoreError;
use std::path::{Path, PathBuf};

/// 把 `repo_url` 淺層(depth=1)clone 進 `clone_into`,回傳 clone 出來的樹裡
/// `packages/<package_name>/` 的絕對路徑。不做真的 sparse-checkout——整個
/// repo 的內容都會被抓下來,只是抓的是最新一次 commit,沒有歷史。
pub fn clone_package_source(
    repo_url: &str,
    package_name: &str,
    clone_into: &Path,
) -> ClientResult<PathBuf> {
    unimplemented!()
}

#[cfg(test)]
mod tests {
    use super::clone_package_source;
    use git2::{Repository, Signature};
    use std::fs;
    use tempfile::tempdir;

    /// 建一個本機 git repo,裡面有一個 `packages/<name>/` 目錄跟一個檔案,
    /// commit 好回傳 repo 的路徑——用來當 `clone_package_source` 的來源,
    /// 完全不需要對外網路連線。
    fn make_source_repo(package_name: &str, file_contents: &str) -> tempfile::TempDir {
        let dir = tempdir().unwrap();
        let repo = Repository::init(dir.path()).unwrap();

        let pkg_dir = dir.path().join("packages").join(package_name);
        fs::create_dir_all(&pkg_dir).unwrap();
        fs::write(pkg_dir.join("packageInfo.json"), file_contents).unwrap();

        let mut index = repo.index().unwrap();
        index
            .add_path(Path::new("packages").join(package_name).join("packageInfo.json").as_path())
            .unwrap();
        index.write().unwrap();
        let tree_id = index.write_tree().unwrap();
        let tree = repo.find_tree(tree_id).unwrap();
        let sig = Signature::now("test", "test@example.com").unwrap();
        repo.commit(Some("HEAD"), &sig, &sig, "initial commit", &tree, &[])
            .unwrap();

        dir
    }

    #[test]
    fn clones_and_finds_the_package_subdirectory() {
        let source_repo = make_source_repo("demo-pkg", r#"{"version":"1.0.0"}"#);
        let dest = tempdir().unwrap();

        let result = clone_package_source(
            source_repo.path().to_str().unwrap(),
            "demo-pkg",
            &dest.path().join("clone"),
        )
        .unwrap();

        assert!(result.ends_with("packages/demo-pkg"));
        assert!(result.join("packageInfo.json").exists());
        let contents = std::fs::read_to_string(result.join("packageInfo.json")).unwrap();
        assert_eq!(contents, r#"{"version":"1.0.0"}"#);
    }

    #[test]
    fn missing_package_subdirectory_is_an_error() {
        let source_repo = make_source_repo("other-pkg", "{}");
        let dest = tempdir().unwrap();

        let result = clone_package_source(
            source_repo.path().to_str().unwrap(),
            "demo-pkg",
            &dest.path().join("clone"),
        );

        assert!(result.is_err(), "demo-pkg was never added to the source repo");
    }
}
```

- [ ] **Step 2: 確認測試失敗(紅燈)**

Run: `cargo test -p DPM source_clone -- --nocapture 2>&1 | tail -30`
Expected:兩個測試都因為 `unimplemented!()` panic 而 FAILED。

- [ ] **Step 3: 實作 `clone_package_source`**

把 `unimplemented!()` 換成:

```rust
pub fn clone_package_source(
    repo_url: &str,
    package_name: &str,
    clone_into: &Path,
) -> ClientResult<PathBuf> {
    let mut fetch_opts = git2::FetchOptions::new();
    fetch_opts.depth(1);
    let mut builder = git2::build::RepoBuilder::new();
    builder.fetch_options(fetch_opts);
    builder
        .clone(repo_url, clone_into)
        .map_err(|e| ClientError::SystemError(format!("git clone of {repo_url} failed: {e}")))?;

    let package_src = clone_into.join("packages").join(package_name);
    if !package_src.is_dir() {
        return Err(ClientError::Core(CoreError::PackageNotFound(format!(
            "{package_name} (no packages/{package_name}/ directory in {repo_url})"
        ))));
    }
    Ok(package_src)
}
```

- [ ] **Step 4: 跑測試,確認變綠**

Run: `cargo test -p DPM source_clone -- --nocapture 2>&1 | tail -30`
Expected: 2 passed。

- [ ] **Step 5: 掛進 `utils/mod.rs`**

編輯 `crates/dpm/src/utils/mod.rs`,加入 `pub mod source_clone;` 跟 `pub use self::source_clone::*;`(照這個檔案裡其他 `utils` 子模組——例如 `db`/`system`/`models`——已經有的同樣掛法照抄,維持風格一致)。

- [ ] **Step 6: 整個 workspace 編譯 + 測試**

Run: `cargo build --workspace 2>&1 | tail -40`
Expected: 無錯誤。

Run: `cargo test -p DPM 2>&1 | tail -60`
Expected: 全部通過,含新的 2 個 `source_clone` 測試。

- [ ] **Step 7: Commit**

```bash
git add crates/dpm/src/utils/source_clone.rs crates/dpm/src/utils/mod.rs
git commit -m "$(cat <<'EOF'
feat(dpm): shallow-clone helper for source-kind package installs

clone_package_source shallow-clones (depth=1) a source's repo and
returns the packages/<pkg>/ subdirectory's path, or PackageNotFound if
that package isn't in the cloned repo. Not true sparse-checkout — see
the plan's "與 spec 的差異" section for why that's an accepted
simplification for now.

Tested against a real local git repo (git2::Repository::init + a
manual commit), not a mock — no network access needed since git2 can
clone from a local filesystem path directly.

Co-Authored-By: Claude Sonnet 5 <noreply@anthropic.com>
EOF
)"
```

---

## Task 4: `install()` 分岔支援 Source 套件

**Files:**
- Modify: `crates/dpm/src/action.rs`

**Interfaces:**
- Consumes:Task 1-3 的 `PackageKind::Source`(已存在)、`clone_package_source`、`DbPackage { kind, build_command, entry, .. }`(Phase 2 已存在)。
- Produces:無新公開介面——`install()` 內部行為改變。

- [ ] **Step 1: 在 `install()` 迴圈裡,`repo_package_info` 解出來之後、原本假設 Prebuilt 的程式碼之前,插入 `kind` 分岔**

編輯 `crates/dpm/src/action.rs`,`install()` 方法裡,在:

```rust
                let repo_package_info = get_db()
                    .latest_version(&source_alias, pkg)
                    .await
                    .map_err(|e| ClientError::Core(CoreError::DatabaseError(e.to_string())))?
                    .ok_or_else(|| {
                        ClientError::Core(CoreError::PackageNotFound(pkg.to_string()))
                    })?;
```

這段之後、`if self.verbose { println!("{}\n\n  {}", pkg.on_green(), "Downloading...".yellow()); }` 之前,插入:

```rust
                let staging_root_base = MAIN_DIR.get().unwrap().join(".staging");
                std::fs::create_dir_all(&staging_root_base)
                    .map_err(|e| ClientError::Core(CoreError::IoError(e)))?;
                let staging = tempfile::Builder::new()
                    .prefix(pkg)
                    .tempdir_in(&staging_root_base)
                    .map_err(|e| ClientError::Core(CoreError::IoError(e)))?;

                if repo_package_info.kind == "source" {
                    self.install_source_package(pkg, &source_alias, &repo_package_info, &staging)?;
                    if self.verbose {
                        println!("  {}", "Installed!".green());
                    }
                    continue;
                }
```

(這段取代掉原本重複出現在 `install()` 裡的 `staging_root_base`/`staging` 建立那兩行——它們本來在 Prebuilt 分支裡建立一次,現在提前到分岔之前,兩個分支共用同一份 staging 目錄建立邏輯,不用各自重複寫一次。原本 Prebuilt 分支裡建 `staging_root_base`/`staging` 的那兩行——`let staging_root_base = ...`/`let staging = ...`——要跟著刪除,因為現在提前到前面共用了。)

- [ ] **Step 2: 加 `install_source_package` 私有方法**

在 `impl ActionInfo` 區塊內(跟 `install`/`update`/`source` 等方法同一層)加入:

```rust
    /// 安裝一個 `kind: "source"` 的套件:淺層 clone 它的來源 repo、在 staging
    /// 目錄裡用呼叫者當下的權限(不經過 `system_command_runner`,所以不管
    /// `--system` 與否都不會提權)執行 `build_command`,`$OUT` 指向這次的產出
    /// 目錄,成功後透過既有的 `swap_into_install_dir` 原子換裝。
    fn install_source_package(
        &self,
        pkg: &str,
        source_alias: &str,
        repo_package_info: &DbPackage,
        staging: &tempfile::TempDir,
    ) -> ClientResult<()> {
        if source_alias != "official" {
            println!(
                "{} installing a source package from a third-party source, not vetted by the DPM team",
                "Warning:".yellow()
            );
        }

        let build_command = repo_package_info.build_command.clone().ok_or_else(|| {
            ClientError::Core(CoreError::InvalidPackage(format!(
                "{pkg} is kind=source but has no build command recorded"
            )))
        })?;

        let sources = self.setting_config.sources.clone();
        let source = sources
            .iter()
            .find(|s| s.alias == source_alias)
            .ok_or_else(|| {
                ClientError::ConfigError(format!("source '{source_alias}' is not configured"))
            })?;

        if self.verbose {
            println!("  {}", "Fetching source...".yellow());
        }
        let clone_dir = staging.path().join("clone");
        let package_src = clone_package_source(&source.repo_url, pkg, &clone_dir)?;

        let out_dir = staging.path().join("out");
        std::fs::create_dir_all(&out_dir).map_err(|e| ClientError::Core(CoreError::IoError(e)))?;

        if self.verbose {
            println!("  {}", "Building (running an untrusted build command)...".yellow());
        }
        let status = std::process::Command::new("sh")
            .arg("-c")
            .arg(&build_command)
            .current_dir(&package_src)
            .env("OUT", &out_dir)
            .status()
            .map_err(|e| ClientError::SystemError(format!("failed to run build command: {e}")))?;
        if !status.success() {
            return Err(ClientError::SystemError(format!(
                "build command for {pkg} exited with {status}"
            )));
        }

        let install_path = INSTALL_DIR.get().unwrap().join(pkg);
        swap_into_install_dir(&out_dir, &install_path, staging.path())?;

        if !repo_package_info.entry.is_empty() {
            let main_file = install_path.join(&repo_package_info.entry);
            let ln_path = BIN_DIR.get().unwrap().join(pkg);
            fs::set_permissions(&main_file, Permissions::from_mode(0o755))
                .map_err(|e| ClientError::SystemError(e.to_string()))?;
            self.system_controller.system_command_runner(
                "ln",
                vec![
                    "-s",
                    main_file.display().to_string().as_str(),
                    ln_path.display().to_string().as_str(),
                ],
                "Can't create link",
            )?;
        }
        Ok(())
    }
```

(`std::process::Command::new("sh").arg("-c").arg(&build_command)` 是刻意直接呼叫,不經過 `self.system_controller.system_command_runner(...)`——那個函式在 `--system` scope 下會自動加 `sudo` 前綴,build 指令絕對不可以走那條路,見本 plan 的 Global Constraints。`swap_into_install_dir`/建立 `BIN_DIR` symlink 那段沿用既有機制,跟 Prebuilt 分支的收尾邏輯一致。)

- [ ] **Step 3: 補 `use`**

檔案頂端 `use` 區塊補上 `clone_package_source`(從 `crate::utils::*` 或既有的 import 路徑,跟 `read_file_from_zip`/`unzip_file` 那些既有 utils 函式一樣的引入方式)、`DbPackage`(如果還沒 import)。

- [ ] **Step 4: 整個 workspace 編譯 + 測試**

Run: `cargo build --workspace 2>&1 | tail -40`
Expected: 無錯誤。

Run: `cargo test --workspace 2>&1 | tail -80`
Expected: 全部通過(含 Task 3 的 `source_clone` 測試、既有的 `atomic_install_tests`/`db_tests`/`config_tests`/`cli_parse_tests`)。

- [ ] **Step 5: 手動端到端驗證(沒有自動測試涵蓋整個 `install()` 流程,原因跟先前幾個 phase 的 `init()`/`config.json` 手動驗證說明一樣——`MAIN_DIR`/`INSTALL_DIR`/`DB_INSTANCE` 都是行程全域的 `OnceLock`,沒有可注入的測試 seam)**

用 Task 1 建立的本機 `dpm-server` 資料(`source-demo` 套件,`kind: source`)當來源——把 `dpm-server` 那個暫存 repo 目錄本身初始化成一個 git repo(這樣 `dpm` 才有東西可以 clone),然後設定 `dpm` 指向它:

```bash
TMPDIR=$(mktemp -d)
cd "$TMPDIR"
git init -q
BIN="/Users/derrick/Documents/Program/rust/Project/DPM-Workspace/target/debug/dpm-server"
cargo build --manifest-path /Users/derrick/Documents/Program/rust/Project/DPM-Workspace/Cargo.toml -p DPM-Server
"$BIN" init source-demo main.py -v 1.0.0 -d "a source-kind demo"
"$BIN" fix add source-demo --build "cp main.py \$OUT/main.py"
git add -A && git commit -q -m "test repo for source-kind install"

DPM_HOME=$(mktemp -d)
DPM_BIN="/Users/derrick/Documents/Program/rust/Project/DPM-Workspace/target/debug/dpm"
cargo build --manifest-path /Users/derrick/Documents/Program/rust/Project/DPM-Workspace/Cargo.toml -p DPM
HOME="$DPM_HOME" "$DPM_BIN" source remove official 2>/dev/null || true
HOME="$DPM_HOME" "$DPM_BIN" source add "file://$TMPDIR" --as local-test
HOME="$DPM_HOME" "$DPM_BIN" update
HOME="$DPM_HOME" "$DPM_BIN" install source-demo
find "$DPM_HOME" -iname "*source-demo*"
```

Expected:最後的 `find` 找得到安裝進 `Software/source-demo/main.py` 的檔案(內容跟原始 `main.py` 一致)——證明 clone → build(`cp` 指令)→ 原子換裝整條路徑真的跑通了。如果這個環境的沙盒限制導致 `HOME` 覆寫或本機 `file://` clone 有問題,改成只確認 `cargo build --workspace`/`cargo test --workspace` 全綠,並在 commit message/報告裡註明手動端到端驗證因環境限制沒能跑,不要跳過測試 Step 4 的自動化部分。

- [ ] **Step 6: Commit**

```bash
git add crates/dpm/src/action.rs
git commit -m "$(cat <<'EOF'
feat(dpm): install() supports kind=source packages

New branch in install(): shallow-clones the package's source repo
(clone_package_source), runs its recorded build_command via a plain
std::process::Command — deliberately NOT through
system_command_runner, so it never gets sudo-prefixed under --system
scope — with $OUT pointing at a staging output directory, then
atomically swaps that output into place via the same
swap_into_install_dir Prebuilt already uses. Warns before installing
from any non-official source, matching `source add`'s existing
warning wording.

Co-Authored-By: Claude Sonnet 5 <noreply@anthropic.com>
EOF
)"
```

---

## Task 5: 收尾——文件 + 全 workspace 驗證

**Files:**
- Modify: `TODO.md`

**Interfaces:** 無。

- [ ] **Step 1: 更新 `TODO.md`,把已經解決的 `git2` 死依賴項目打勾**

編輯根目錄 `TODO.md`,找到「`dpm` 的 `Cargo.toml` 有多個宣告但完全沒用到的 dependency」那條(P2 重複/死碼段),把其中提到 `git2` 的部分改成已解決(這個項目原本列了好幾個死依賴一起講,`git2` 這次真的被用到了,其餘如 `rusqlite`/`dotenv`/`digest`/`flate2` 維持原樣不動,不在這個 phase 範圍內)。

- [ ] **Step 2: 整個 workspace 最終編譯 + 測試**

Run: `cargo build --workspace 2>&1 | tail -40`
Expected: 無錯誤。

Run: `cargo test --workspace 2>&1 | tail -100`
Expected: 全部測試通過——這個 plan 新增的所有測試(`source_clone` 2 個)加上先前所有 phase 累積下來的既有測試全部一起綠燈。

- [ ] **Step 3: Commit**

```bash
git add TODO.md
git commit -m "$(cat <<'EOF'
docs(todo): mark git2 dead-dependency item resolved

git2 is now genuinely used by clone_package_source (Task 3 of the
client-source-install plan) — no longer dead weight. The other
dependencies this TODO item originally bundled together (rusqlite,
dotenv, digest, flate2) are untouched by this plan and stay as
separate future cleanup.

Co-Authored-By: Claude Sonnet 5 <noreply@anthropic.com>
EOF
)"
```
