# dpm-server 發布模型改版(Phase 3, 程式碼部分) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** `dpm-server` 不再把打包好的 `.zip` 存進自己的 git repo(`Repo/<pkg>.zip`)——`fix add` 改成吃一個外部 URL,由 `dpm-server` 自己下載一次算 blake3 hash 寫進 `RepoInfo.json`,不留副本。這是 `docs/superpowers/specs/2026-07-24-multi-source-registry-design.md` Section 6-7 的**程式碼部分**(Section 6:發布模型;Section 7:CI/CD 的 YAML 檔本身)。

**Architecture:** 三個循序 task。Task 1 把 `Repo/src/<pkg>/` 這個本地開發目錄改名成 `packages/<pkg>/`(純路徑字串,無行為變化)。Task 2 是核心:`fix_add` 從「讀本地 `Repo/<pkg>.zip` 算 hash + 硬編 GitHub raw URL」改成「吃 `--url` 下載該外部檔案算 hash」,`repo_init`(靠掃描本地 `.zip` 重建整個索引的舊邏輯)直接刪除——外部託管之後,本地已經沒有足夠資訊能重建索引了,這是這次架構改變的直接結果,不是遺漏。`build` 指令維持原樣(spec 明確說它降級成純本地開發便利工具,跟發布流程脫鉤)。Task 3 寫兩個 GitHub Actions YAML 檔。

**Tech Stack:** Rust 2021、`reqwest`(dpm-server 之前完全不用網路,這次新增 `blocking` feature 做同步下載——`dpm-server` 的 `main()` 目前是同步的,沒有 tokio runtime,不想為了一次 HTTP GET 把整個 CLI 改成 async)、GitHub Actions YAML。

## 與 spec 的差異(刻意的範圍收斂,附理由)

1. **這個 phase 完全不碰 `PackageKind::Source`(build-from-source 發布)。** spec Section 6 把「不 host 二進位」跟「支援 source 套件發布」寫在同一段,但這兩件事可以獨立達成第一件不需要第二件——`dpm-server` 現在唯一在管理的就是 prebuilt 套件,把它們的 hosting 從「自己存一份 zip」改成「外部 URL,自己只算 hash」就已經完全達成「不 host 二進位」這個目標。`Source` 套件在 client 端目前根本裝不了(Phase 4 才做,見 spec 已知風險段的排序),現在就在 `dpm-server` 端做完整的 source 發布支援只是死碼——沒有任何 client 會去裝一個 `kind: source` 的套件。等 Phase 4 落地 client 端 source 安裝路徑,再回頭補 `dpm-server` 這邊的 source 發布支援,兩邊才會同時可用。
2. **`repo_init`(掃描 `Repo/*.zip` 重建整個 `RepoInfo.json`)直接刪除,不是改寫。** 這個函式的前提是「本地有一份完整的 zip 清單可以掃描重建索引」——外部託管之後這個前提不再成立(`url`/`hash` 只在當初執行 `fix add` 時才知道,事後從本地檔案系統掃不出來)。`main()` 裡「`RepoInfo.json` 不存在就跑 `repo_init` 掃描」的分支改成單純建一個空的 `RepoInfo::new()`。
3. **CI YAML 不做 spec Section 7 描述的「PR merge 後 bot 自動執行 `fix add` 並 commit 回 main」。** 那個自動化模型假設 CI 能自己判斷出要發布的套件跟它的 URL——但 `fix add --url` 需要人工提供外部託管網址,CI 沒有管道知道這個值。這個 phase 的現實流程是:維護者在本機跑 `fix add`,把 `packages/<pkg>/` 的變動**連同** `RepoInfo.json` 的變動一起放進同一個 PR;`pr-check.yml` 的工作變成「檢查這個 PR 對 `RepoInfo.json` 的改動有沒有動到既有版本」(已發布版本不可變,這是 Rust 層 `add_package_version` 已經擋掉的事,但那個擋只在單次執行內有效——PR 裡直接手改 JSON 檔繞過它是可能的,CI 要在這層再擋一次)。`publish.yml` 的角色相應變小:merge 進 main 後只做一次事後複查(defense in depth),不做任何寫回動作。
4. **這兩個 YAML 檔寫出來之後,不會、也不能在這個環境裡真的跑。** 這個 repo 目前沒有連到任何真實的 GitHub remote(`git remote -v` 是空的——`DPM-Server`/`DPM-Core`/`DPM` 是三個歷史上獨立的 repo,現在整併進這個 workspace,原本各自的 GitHub URL 只是程式碼裡殘留的字串)。要讓這兩個 workflow 真的觸發,需要:(a) 這個 repo 真的推到 GitHub;(b) `publish.yml` 需要一把 fine-grained PAT 存進 repo secret;(c) branch protection 設「`pr-check.yml` 的 `verify-index` job 必須通過才能 merge」。這三件事都是 GitHub 網頁操作,不是這個 plan 的 task,需要人工完成。

## Global Constraints

- 這個 phase 不新增 `semver`/`pubgrub` 依賴。
- `PackageKind::Source` 相關的 `dpm-server` 端支援不在這個 phase 做(見上方差異點 1)。
- `reqwest` 的 `blocking` feature 只加在 `dpm-server` 自己的 `Cargo.toml`(用 `{ workspace = true, features = ["blocking"] }` 疊加,不動根 `Cargo.toml` 的 workspace 共用 feature 列表,`dpm`/`dpm-core` 不需要這個 feature)。
- 每個 task 完成後執行 `cargo build --workspace` 確認整個 workspace 仍能編譯;有新增/修改測試的 task 額外跑 `cargo test --workspace`。
- `build` 指令(本地開發便利工具,產出 `Repo/<pkg>.zip`)維持原樣,不動。

---

## Task 1: `Repo/src/<pkg>/` 改名 `packages/<pkg>/`

**Files:**
- Modify: `crates/dpm-server/src/main.rs`
- Modify: `crates/dpm-server/src/action.rs`

**Interfaces:**
- Consumes:無。
- Produces:無新介面——純路徑字串改名,`PROJECT_SRC`(`static OnceLock<PathBuf>`)的值從 `<cwd>/Repo/src` 改成 `<cwd>/packages`,型別跟用法不變。

- [ ] **Step 1: 改 `main.rs` 的 `PROJECT_SRC` 設定**

編輯 `crates/dpm-server/src/main.rs`,把:

```rust
    let repo_src = current_dir()?.join("Repo/src");
```

改成:

```rust
    let repo_src = current_dir()?.join("packages");
```

(這行下面緊接著的 `PROJECT_SRC.set(repo_src.clone()).unwrap();`、`create_dir_all(repo_src)?;` 不用動,邏輯不變,只是路徑不同。)

- [ ] **Step 2: 改 `action.rs::init()` 裡重複的路徑字串**

編輯 `crates/dpm-server/src/action.rs`,`init()` 函式裡:

```rust
    let project_path = current_dir()
        .unwrap()
        .join("Repo/src")
        .join(obj.name.as_str());
```

改成直接用 `PROJECT_SRC`(跟 `main.rs` 共用同一個值,順手去掉這裡原本自己重新組一次路徑字串的重複):

```rust
    let project_path = PROJECT_SRC.get().unwrap().join(obj.name.as_str());
```

- [ ] **Step 3: 改 `action.rs::fix_add()` 裡的路徑字串**

`fix_add` 函式裡:

```rust
    let path = std::env::current_dir()?
        .join("Repo/src")
        .join(&obj.project_name);
```

改成:

```rust
    let path = PROJECT_SRC.get().unwrap().join(&obj.project_name);
```

(這一步跟 Task 2 會再動到 `fix_add` 的其他部分——這裡先只处理路徑,Task 2 再處理 URL/hash 那段邏輯,避免一個 task 做太多事。)

- [ ] **Step 4: 整個 workspace 編譯確認**

Run: `cargo build --workspace 2>&1 | tail -40`
Expected: 無錯誤——這個 task 純粹是路徑字串改名,不涉及任何簽名/型別變化,應該完全不影響編譯結果。

- [ ] **Step 5: 手動驗證行為(沒有自動測試涵蓋這個路徑,`dpm-server` 目前完全沒有 `tests/` 目錄)**

Run(在一個乾淨的暫存目錄裡,不要在這個 repo 目錄本身跑,避免弄髒 working tree):

```bash
TMPDIR=$(mktemp -d)
cd "$TMPDIR"
cargo run --manifest-path /Users/derrick/Documents/Program/rust/Project/DPM-Workspace/Cargo.toml -p DPM-Server -- init demo-pkg main.py
ls packages/demo-pkg/
```

Expected:`packages/demo-pkg/` 目錄存在(不是 `Repo/src/demo-pkg/`),裡面有 `main.py`、`hashes.json`、`packageInfo.json`。跑完清掉 `$TMPDIR`。

- [ ] **Step 6: Commit**

```bash
git add crates/dpm-server/src/main.rs crates/dpm-server/src/action.rs
git commit -m "$(cat <<'EOF'
refactor(dpm-server): rename Repo/src/<pkg>/ to packages/<pkg>/

Pure path-string rename, no behavior change — packages/ is the
directory name Section 6 of the multi-source registry design settles
on for per-package source files (packageInfo.json/hashes.json), since
the next task removes the "Repo/ also holds a hosted .zip per
package" half of the old layout.

Also collapsed init()'s and fix_add()'s independent
current_dir().join("Repo/src") path construction into the shared
PROJECT_SRC static both functions already had available.

Co-Authored-By: Claude Sonnet 5 <noreply@anthropic.com>
EOF
)"
```

---

## Task 2: `fix add` 改吃外部 URL,不再本地託管 zip;刪除失效的 `repo_init`

**Files:**
- Modify: `crates/dpm-server/Cargo.toml`
- Modify: `crates/dpm-server/src/cli_parse.rs`
- Modify: `crates/dpm-server/src/action.rs`
- Modify: `crates/dpm-server/src/main.rs`

**Interfaces:**
- Consumes:Phase 2 的 `dpm_core::{RepoInfo::add_package_version, PackageVersionInfo, PackageKind}`(已存在,不用改)。
- Produces:`Add` clap struct 新增 `--url`(必填)、`--file-name`(可選,預設從 URL 最後一段路徑推導)兩個欄位。`fix_add` 不再讀 `Repo/<pkg>.zip`,改成下載 `--url` 指向的檔案算 hash。`repo_init`/`find_zip_files_and_names_in_repo` 兩個函式整個刪除。

- [ ] **Step 1: `reqwest` 加 `blocking` feature**

編輯 `crates/dpm-server/Cargo.toml`,`[dependencies]` 區塊加一行(維持字母序,插在 `hex-literal` 之後、`serde` 之前):

```toml
reqwest = { workspace = true, features = ["blocking"] }
```

- [ ] **Step 2: `Add` clap struct 加 `--url`/`--file-name`**

編輯 `crates/dpm-server/src/cli_parse.rs`,把現有的:

```rust
#[derive(Args, Debug)]
pub struct Add {
    /// Project Name
    pub project_name: String,
}
```

換成:

```rust
#[derive(Args, Debug)]
pub struct Add {
    /// Project Name
    pub project_name: String,
    /// External URL hosting the prebuilt package archive. dpm-server
    /// downloads it once to compute its blake3 hash — it does not keep a
    /// copy. Must be https://.
    #[arg(long)]
    pub url: String,
    /// Override the file name recorded in RepoInfo.json (defaults to the
    /// URL's last path segment, e.g. "foo.zip" from ".../foo.zip")
    #[arg(long)]
    pub file_name: Option<String>,
}
```

- [ ] **Step 3: 重寫 `fix_add`,下載外部 URL 算 hash**

編輯 `crates/dpm-server/src/action.rs`,把 `fix_add`(Task 1 Step 3 已經改過路徑那版)整個換成:

```rust
fn fix_add(obj: &Add, repo: &mut RepoInfo) -> Result<()> {
    let path = PROJECT_SRC.get().unwrap().join(&obj.project_name);
    let pk_info: PackageInfo = JsonStorage::from_json(&path.join("packageInfo.json"))?;

    if !obj.url.starts_with("https://") {
        return Err(anyhow::anyhow!(
            "\n--url {} {}",
            obj.url.yellow(),
            "must use https://".red()
        ));
    }
    let file_name = obj
        .file_name
        .clone()
        .or_else(|| obj.url.rsplit('/').next().map(|s| s.to_string()))
        .filter(|s| !s.is_empty())
        .ok_or_else(|| {
            anyhow::anyhow!("could not derive a file name from --url; pass --file-name explicitly")
        })?;

    let response = reqwest::blocking::get(&obj.url)?;
    if !response.status().is_success() {
        return Err(anyhow::anyhow!(
            "\nfailed to fetch {}: HTTP {}",
            obj.url.yellow(),
            response.status()
        ));
    }
    let bytes = response.bytes()?;
    let tmp_path = std::env::temp_dir().join(&file_name);
    std::fs::write(&tmp_path, &bytes)?;
    let hash = dpm_core::hash_file(&tmp_path)?;
    std::fs::remove_file(&tmp_path)?;

    let version_info = PackageVersionInfo {
        version: pk_info.version.clone(),
        kind: PackageKind::Prebuilt {
            url: obj.url.clone(),
            hash,
            file_name,
        },
        dependencies: pk_info.dependencies,
        entry: None,
        description: Some(pk_info.description),
    };
    repo.add_package_version(obj.project_name.clone(), version_info)?;
    Ok(())
}
```

(拿掉原本用 `dpm_core::hash_file(&package)` 讀本地 zip、跟拿掉硬編的 `format!("https://github.com/.../Repo/{}.zip", ...)` URL 組字串——這兩件事現在都由呼叫端透過 `--url` 直接提供。)

- [ ] **Step 4: 刪除 `repo_init`/`find_zip_files_and_names_in_repo`**

在 `crates/dpm-server/src/action.rs` 裡整個刪除 `pub fn repo_init(repo: &mut RepoInfo) -> Result<()> { ... }` 跟 `fn find_zip_files_and_names_in_repo() -> Result<Vec<(PathBuf, String)>> { ... }` 這兩個函式(理由見本 plan 開頭「與 spec 的差異」第 2 點——這兩個函式的前提在這次改動後不再成立)。

檢查檔案頂端 `use` 區塊:`read_dir`(只有 `find_zip_files_and_names_in_repo` 用到)如果變成 unused import,一併刪除。

- [ ] **Step 5: `main.rs` 不再呼叫 `repo_init`**

編輯 `crates/dpm-server/src/main.rs`,把:

```rust
    if !software_repo_info.exists() {
        println!("RepoInfo.json not found. Initializing a new one.");
        repo_info = RepoInfo::new();
        repo_init(&mut repo_info)?;
    } else {
```

改成:

```rust
    if !software_repo_info.exists() {
        println!("RepoInfo.json not found. Initializing an empty one.");
        repo_info = RepoInfo::new();
    } else {
```

- [ ] **Step 6: 整個 workspace 編譯確認**

Run: `cargo build --workspace 2>&1 | tail -60`
Expected: 無錯誤。

- [ ] **Step 7: 手動驗證 `fix add` 端到端行為**

沒有自動測試涵蓋這條路徑(`dpm-server` 沒有 `tests/` 目錄,新增測試需要一個真實可下載的 HTTPS URL,不適合寫進單元測試)。手動驗證(在暫存目錄裡跑,不要弄髒這個 repo):

```bash
TMPDIR=$(mktemp -d)
cd "$TMPDIR"
BIN="/Users/derrick/Documents/Program/rust/Project/DPM-Workspace/target/debug/dpm-server"
cargo build --manifest-path /Users/derrick/Documents/Program/rust/Project/DPM-Workspace/Cargo.toml -p DPM-Server
"$BIN" init demo-pkg main.py -v 1.0.0 -d "demo package"
"$BIN" fix add demo-pkg --url https://raw.githubusercontent.com/rust-lang/rust/master/README.md
cat RepoInfo.json
```

Expected:`RepoInfo.json` 裡 `packages.demo-pkg` 是一個陣列,唯一元素有 `"kind": "prebuilt"`、`"url": "https://raw.githubusercontent.com/rust-lang/rust/master/README.md"`、`"file_name": "README.md"`、一個真的算出來的 blake3 hash(64 個十六進位字元)。跑完清掉 `$TMPDIR`。

如果這個環境沒有對外網路連線導致這步驟沒辦法真的下載,改成: Run: `cargo build -p DPM-Server 2>&1 | tail -20` 確認至少編譯得過,並在 commit message/PR 描述裡註明這步驟需要對外網路、CI 環境需要確認可以連到任意 HTTPS URL。

- [ ] **Step 8: Commit**

```bash
git add crates/dpm-server/Cargo.toml crates/dpm-server/src/cli_parse.rs \
  crates/dpm-server/src/action.rs crates/dpm-server/src/main.rs
git commit -m "$(cat <<'EOF'
feat(dpm-server): fix add downloads an external URL instead of hosting a zip

dpm-server no longer stores a copy of any package's binary — `fix add`
now takes --url (required) pointing at wherever the package is
actually hosted, downloads it once to compute its blake3 hash, and
never keeps the download. This is the core of Section 6's "don't host
any binaries" model.

repo_init (rebuild RepoInfo.json by rescanning local Repo/*.zip files)
and its helper are deleted — that rebuild's entire premise (a
recoverable local zip listing) no longer holds once hosting is
external. main()'s first-run path now just creates an empty RepoInfo
instead of scanning for zips that no longer exist locally.

`build` (local dev convenience, unrelated to publishing) is untouched.

Co-Authored-By: Claude Sonnet 5 <noreply@anthropic.com>
EOF
)"
```

---

## Task 3: GitHub Actions CI/CD YAML(寫出檔案,不代表已經接上真實 GitHub repo)

**Files:**
- Create: `.github/workflows/pr-check.yml`
- Create: `.github/workflows/publish.yml`

**Interfaces:**
- Consumes:無(純 YAML,不影響任何 Rust 程式碼編譯)。
- Produces:無 Rust 介面。這兩個檔案本身是「產出」。

- [ ] **Step 1: 建立 `.github/workflows/pr-check.yml`**

```yaml
name: PR Check

on:
  pull_request:
    paths:
      - 'crates/dpm-server/packages/**'
      - 'crates/dpm-server/RepoInfo.json'

permissions:
  contents: read

jobs:
  verify-index:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
        with:
          fetch-depth: 0

      - name: Every changed package has a packageInfo.json
        run: |
          set -euo pipefail
          git diff --name-only "origin/${{ github.base_ref }}...HEAD" -- crates/dpm-server/packages/ \
            | awk -F/ '{print $4}' | sort -u > /tmp/changed_packages.txt
          cat /tmp/changed_packages.txt
          while read -r pkg; do
            [ -z "$pkg" ] && continue
            info="crates/dpm-server/packages/$pkg/packageInfo.json"
            if [ ! -f "$info" ]; then
              echo "::error::$info is missing"
              exit 1
            fi
          done < /tmp/changed_packages.txt

      - name: Published versions in RepoInfo.json are immutable
        run: |
          set -euo pipefail
          git show "origin/${{ github.base_ref }}:crates/dpm-server/RepoInfo.json" \
            > /tmp/base_repoinfo.json 2>/dev/null || echo '{"packages":{}}' > /tmp/base_repoinfo.json
          python3 - <<'PY'
          import json, sys

          with open("/tmp/base_repoinfo.json") as f:
              base = json.load(f).get("packages", {})
          with open("crates/dpm-server/RepoInfo.json") as f:
              head = json.load(f).get("packages", {})

          errors = []
          for name, base_versions in base.items():
              head_by_version = {v["version"]: v for v in head.get(name, [])}
              for bv in base_versions:
                  hv = head_by_version.get(bv["version"])
                  if hv is not None and hv != bv:
                      errors.append(f"{name}@{bv['version']} was modified — published versions are immutable")

          if errors:
              for e in errors:
                  print(f"::error::{e}")
              sys.exit(1)
          print("OK: no published version was modified")
          PY
```

(這個 workflow 用 `pull_request` trigger,不是 `pull_request_target`,執行時不帶任何 secret、`permissions: contents: read` 是唯讀——PR 內容是不信任輸入,符合 spec Section 7 第 6 點的權限隔離要求。)

- [ ] **Step 2: 建立 `.github/workflows/publish.yml`**

```yaml
name: Publish sanity check

on:
  push:
    branches: [main]
    paths:
      - 'crates/dpm-server/RepoInfo.json'

permissions:
  contents: read

jobs:
  sanity-check:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4

      - name: RepoInfo.json is well-formed JSON with the expected shape
        run: |
          set -euo pipefail
          python3 - <<'PY'
          import json, sys

          with open("crates/dpm-server/RepoInfo.json") as f:
              data = json.load(f)

          packages = data.get("packages")
          if not isinstance(packages, dict):
              print("::error::RepoInfo.json's top-level 'packages' key is missing or not an object")
              sys.exit(1)

          for name, versions in packages.items():
              if not isinstance(versions, list) or not versions:
                  print(f"::error::{name} must map to a non-empty array of versions")
                  sys.exit(1)
              seen = set()
              for v in versions:
                  ver = v.get("version")
                  if ver in seen:
                      print(f"::error::{name} has a duplicate version entry: {ver}")
                      sys.exit(1)
                  seen.add(ver)
                  if v.get("kind") not in ("prebuilt", "source"):
                      print(f"::error::{name}@{ver} has an invalid or missing 'kind'")
                      sys.exit(1)

          print(f"OK: {len(packages)} package(s), all well-formed")
          PY
```

這個 workflow 的角色比 spec Section 7 原本設想的小(見本 plan 開頭「與 spec 的差異」第 3 點)——它不做任何寫回動作(不需要 PAT,`permissions: contents: read` 就夠),純粹是 merge 進 main 之後的事後複查,抓那種「PR 通過了 `pr-check.yml` 但因為某種原因(rebase 衝突、手動 push 繞過 branch protection)main 上的 `RepoInfo.json` 還是壞的」的邊界情況。

- [ ] **Step 3: 用 `actionlint` 或等效工具驗證 YAML 語法(如果環境裡有的話;沒有就跳過,不要為了這步驟新裝工具)**

Run: `command -v actionlint >/dev/null 2>&1 && actionlint .github/workflows/pr-check.yml .github/workflows/publish.yml || echo "actionlint not installed, skipping — YAML syntax will be validated by GitHub itself on first push"`

- [ ] **Step 4: Commit**

```bash
git add .github/workflows/pr-check.yml .github/workflows/publish.yml
git commit -m "$(cat <<'EOF'
ci: add pr-check/publish workflow YAML for dpm-server's package index

pr-check.yml runs on every PR touching crates/dpm-server/packages/ or
RepoInfo.json: confirms every changed package still has a
packageInfo.json, and — the real security-relevant check — confirms
no PR modifies an already-published version's RepoInfo.json entry
(published versions are immutable; the Rust-level guard in
add_package_version only holds within one `fix add` invocation, not
against a PR that hand-edits the JSON directly). Runs on
`pull_request` (not `pull_request_target`), no secrets, read-only
permissions — PR content is untrusted input.

publish.yml is a smaller role than Section 7 of the design spec
originally described (see the plan's "與 spec 的差異" section): since
`fix add --url` needs a human-supplied external URL that CI has no
way to discover on its own, there's no bot-commit-back-to-main step
to automate here. It's a post-merge sanity re-check of RepoInfo.json's
shape, nothing more — no PAT needed.

These workflows are not wired to a live GitHub repo yet — this
workspace has no `git remote` configured. Making them actually run
requires: pushing this repo to GitHub, and turning on branch
protection requiring pr-check.yml's `verify-index` job to pass before
merge. That's a manual GitHub-side step, not part of this plan.

Co-Authored-By: Claude Sonnet 5 <noreply@anthropic.com>
EOF
)"
```
