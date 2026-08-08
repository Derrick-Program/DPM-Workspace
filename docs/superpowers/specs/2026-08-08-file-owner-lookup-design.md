# dpm owns — 檔案反查套件設計

## 背景與動機

已裝套件的每個檔案都屬於某個套件,但目前沒有指令能反查「這個檔案是哪個套件裝的」(對照 `dpkg -S`、`pacman -Qo`)。`installed_files` 表(`crates/dpm/src/utils/db.rs`)其實已經記錄這個對應關係——`record_installed_files`/`get_installed_files`/`remove_installed_files` 都已存在,只是沒有開放使用者指令查詢。

TODO.md「功能缺口 — 第二輪」清單中成本最低的一項。

## 目標

- 新增 `dpm owns <path>...` 指令,輸入一或多個檔案路徑,印出各自屬於哪個(些)已裝套件。
- 查無對應套件時印出提示,不中斷其他路徑、不影響 exit code(跟 `info()` 對「查無此套件」的處理一致)。

## 非目標

- 不查 `Software/<pkg>/` 私有安裝目錄裡的原始檔案本身——`installed_files` 只記錄 DPM 建的 symlink(`opt/`、`bin/`、`sbin/`、`lib/`、`share/<pkg>/` 等環境目錄下的連結,見 `placer.rs::place_package`/`link_subdirs_to_env`),這次的查詢範圍就限定在這些 symlink,不額外掃描或記錄套件私有目錄下的個別檔案。已在設計討論中跟使用者確認。
- 不做模糊/子字串比對——只做正規化後的絕對路徑精確比對,避免不同 namespace 下同名檔案(例如兩個套件的 `share/<pkg>/README`)誤判命中。
- 不解析 symlink 目標(不用 `fs::canonicalize`)——`installed_files` 存的是 DPM 建的 symlink 本身的路徑字串,不是它指向的目標;對輸入路徑做符號連結解析會讓它變成 `Software/<pkg>/...` 之類的目標路徑,反而跟表裡存的字串對不上。

## 架構

### `Db::find_owners`(新方法,`crates/dpm/src/utils/db.rs`)

```rust
pub async fn find_owners(&self, file_path: &str) -> ClientResult<Vec<String>> {
    let conn = self.connect().await?;
    let mut rows = conn
        .query(
            "SELECT package_name FROM installed_files WHERE file_path = ?1",
            [file_path],
        )
        .await
        .map_err(|e| ClientError::Core(DatabaseError(e.to_string())))?;
    // rows.next() 迴圈收集 package_name,照抄 get_installed_files 的迴圈寫法
}
```

回傳 `Vec<String>` 而不是 `Option<String>`——`installed_files` 的 PRIMARY KEY 是 `(package_name, file_path)`,理論上兩個套件可以登記同一個 `file_path`(namespace share 情境,`link_subdirs_to_env` 的 `is_namespaced` 分支目前把 `share`/`docs`/`etc`/`var` 底下的檔案分到各自的 `<pkg>/` 子目錄避免碰撞,但這條保護只涵蓋 `is_namespaced=true` 的分類;`bin`/`sbin`/`lib`/`include`/`completions` 沒有這層保護,不同套件仍可能把同名檔案連到同一個 `file_path`),回傳清單比回傳單一結果更誠實。

### CLI 指令(`cli_parse.rs::Commands`)

```rust
/// Show which installed package owns a file
#[command(visible_aliases = ["of"], arg_required_else_help = true)]
Owns {
    #[arg(value_name = "File path", required = true)]
    pn: Vec<String>,
    #[arg(short, long)]
    verbose: bool,
},
```

`pn` 欄位名沿用其他子指令(`Install`/`Uninstall`/`Search`/`Info`)的既有慣例,即使這裡語意是路徑不是套件名——`ActionInfo::new` 只認位置,不看欄位名,沿用同名可以讓 `lib.rs` 的 dispatch 完全照抄既有分支,不需要另開一條建構路徑。

### `action.rs::ActionInfo::owns`

```rust
pub async fn owns(&self) -> ClientResult<()> {
    for raw_path in &self.pkgs {
        let absolute = std::path::absolute(raw_path)
            .map_err(|e| ClientError::Core(CoreError::IoError(e)))?;
        let owners = self.ctx.db.find_owners(&absolute.display().to_string()).await?;
        if owners.is_empty() {
            println!("{}", format!("{raw_path}: not owned by any installed package").yellow());
        } else {
            println!("{}: {}", raw_path, owners.join(", ").bold());
        }
    }
    Ok(())
}
```

`std::path::absolute()`(stable since Rust 1.79,已確認目前工具鏈 1.97 可用)只做 lexical 正規化(補上 cwd、消掉 `.`/`..` component),不觸碰檔案系統、不解析 symlink——跟 `installed_files` 存字串(`opt_link.display().to_string()` 等,同樣是未解析符號連結的絕對路徑)的形式一致。

### `lib.rs` dispatch

```rust
Some(Commands::Owns { pn, verbose }) => {
    ActionInfo::new(ctx.clone(), pn, verbose, setting_config)
        .owns()
        .await?
}
```

照抄 `Commands::Info` 那個分支,插入同一個 `match` 區塊。

## 資料流

`dpm owns /usr/local/bin/hello` → `std::path::absolute` 正規化(若已是絕對路徑則等同原樣)→ `find_owners("/usr/local/bin/hello")` → 查到 `installed_files` 裡 `package_name = "hello"` 那列(裝 `hello` 時 `place_package` 的 entry-point symlink 步驟寫入)→ 印 `"/usr/local/bin/hello: hello"`。

查無結果(例如路徑打錯,或指向套件私有安裝目錄裡未被連結出來的檔案)→ 印黃字提示,continue。

## 錯誤處理

沿用現有 `ClientError`/`ClientResult` 模式,不新增 error variant:

- `std::path::absolute()` 失敗(理論上只在極端環境如拿不到 cwd 時發生)→ 映射成 `ClientError::Core(CoreError::IoError(...))`,跟其他 fs 相關錯誤處理一致,直接讓整個指令以錯誤結束(不是「這個路徑略過、繼續下一個」——這種失敗代表環境本身有問題,不是單純查無結果)。
- `find_owners` 的 DB 查詢失敗 → `?` 正常往上傳播。

## 測試計畫

- `db.rs`:裝兩個套件個別的 `installed_files`(呼叫 `record_installed_files`),`find_owners` 對各自路徑回傳正確的單一套件名;查詢不存在的路徑回傳空 `Vec`;兩個套件登記同一個 `file_path` 時 `find_owners` 回傳兩個名字(namespace share 情境)。
- `action.rs`(比照 `info()`/`list()` 既有的 `Context::for_test` fixture 風格):完整跑一次 `install()`,對它的 entry-point symlink 路徑呼叫 `owns()`,確認印出正確套件名;對不存在的路徑呼叫 `owns()`,確認印出「not owned by any installed package」而不是報錯或 panic。

## 驗證清單

- [ ] `cargo check --workspace`/`cargo clippy --workspace --all-targets -- -D warnings`/`cargo test --workspace` 通過
- [ ] 新單元測試涵蓋上述情境
- [ ] 手動驗證:裝一個套件,`dpm owns <它的 bin symlink 路徑>` 印出正確套件名;`dpm owns /不存在的路徑` 印出查無結果提示
- [ ] TODO.md「功能缺口 — 第二輪」的檔案反查套件項目打勾
