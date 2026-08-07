# dpm autoremove — 孤兒依賴清理設計

## 背景與動機

`InstalledPackages` 現在把「使用者用 `dpm install X` 主動要的套件」跟「因為是別的套件的依賴、被 `resolve_install_set` 一起解出來裝上的套件」存成一模一樣的資料——沒有任何欄位區分兩者。使用者 `dpm uninstall` 掉一個套件之後,它專屬的依賴會永遠留在系統上,沒有任何機制找出來、也沒有指令能清掉,這是套件管理器的基本功能缺口(對照 apt `autoremove`、pacman `-Rs`、dnf `autoremove`)。

TODO.md「功能缺口」清單第一項。

## 目標

- `InstalledPackages` 加一個 `explicit` 欄位,標記這個套件是使用者主動要的(`1`)還是被依賴拉進來的(`0`)。
- `dpm install` 時依「這次命令列有沒有直接指名這個套件」寫入 `explicit`;已經是 `explicit=0` 的套件如果之後被使用者直接指名安裝,升級成 `explicit=1`(一旦升級,不會再自動降回 `0`)。
- 新增純函式 `find_orphans`:找出「`explicit=0` 且沒有任何已裝套件依賴它」的套件,遞迴到 fixpoint(拿掉一輪孤兒後,可能讓它自己的 auto 依賴下一輪也變孤兒)。
- `dpm uninstall` 完成後,若因此產生新孤兒,印出提示(不動手清)。
- 新增 `dpm autoremove` 指令:列出所有孤兒套件並直接清除(沒有孤兒就印出「無孤兒套件」,不需要 `--yes` 之類的二次確認,跟現有 `install`/`uninstall` 的無互動風格一致)。

## 非目標

- 不做「反向操作」——沒有指令可以把 `explicit=1` 手動降回 `0`(對照 `apt-mark auto`)。目前只有「升級」方向,降級不在這次範圍內,以後真的有需求再加。
- 不改動 `resolve_install_set`/pubgrub 解析邏輯本身——`explicit` 完全是安裝完成後,寫入 `InstalledPackages` 那一步才決定的旗標,不影響依賴解析過程。
- 不引入互動式確認(`dialoguer` 之類)——這次維持專案現有的「直接做、不問」慣例,`autoremove` 一樣是列出清單就直接清。
- 不順帶補 geni 版本化 migration 機制——`CLAUDE.md` 目前對 migration 機制的敘述已經跟現在的 `run_migrations()`(直接 `CREATE TABLE IF NOT EXISTS`)對不上,是既有的文件過期問題,不在這次 autoremove 的範圍內處理。

## 架構

### `InstalledPackages` schema 變更

```sql
ALTER TABLE InstalledPackages ADD COLUMN explicit INTEGER NOT NULL DEFAULT 1;
```

`run_migrations(is_info=false)` 在原本的 `CREATE TABLE IF NOT EXISTS InstalledPackages (...)`(不含 `explicit`)之後,額外執行這行 `ALTER TABLE`,並吃掉「欄位已存在」的錯誤(比照現有 `CREATE TABLE IF NOT EXISTS` 的 idempotent 精神——`turso`/SQLite 的 `ALTER TABLE ADD COLUMN` 沒有 `IF NOT EXISTS` 語法,要靠錯誤訊息字串比對來判斷「已經加過」,同一個模式在 `action.rs::is_signature_error` 已經有先例)。

新表(`CREATE TABLE IF NOT EXISTS` 第一次建立)直接把 `explicit` 欄位寫進去,`ALTER TABLE` 對這種情況會是「欄位已存在」直接跳過,不衝突。

既有 DB 檔案(這次改動之前就裝過套件的使用者)所有既有列在 `ALTER TABLE` 的 `DEFAULT 1` 下自動變成 `explicit=1`——即使裡面其實有原本是依賴的套件,這次升級後也視為「使用者要的」,不會被誤判成孤兒清掉。

### `DbPackage`/`COLUMNS` 更新

`crates/dpm/src/utils/db.rs` 的 `COLUMNS` 常數、`crates/dpm/src/utils/models.rs::DbPackage` 都加上 `explicit: bool`,`row_to_package`/`insert`/`DbPackage::new` 照現有模式(逐一對應欄位)更新。`insert_available`/`AvailablePackages` 那條路徑不受影響——`explicit` 只對「已安裝」語意有意義,遠端索引快取不需要這個欄位。

### 寫入 / 升級 `explicit`

`action.rs::install()` 呼叫 `resolve_install_set(all_packages, requests)` 後,已經有原始 `requests`(使用者命令列直接打的 `(source_hint, name, constraint)`)跟 pubgrub 解出來的完整 `(source, name, version)` 清單。對每一個要寫進 `InstalledPackages` 的套件:

- `(source, name)` 出現在 `requests` 裡 → `explicit = true`。
- 否則(pubgrub 額外解出來的依賴) → `explicit = false`,除非這個 `(source, name)` 在 `InstalledPackages` 裡已經有一列且 `explicit = true`(維持原本的 `true`,不因為這次又被別人依賴到就降級——降級本來就不在這次範圍內)。
- 如果 `(source, name)` 這次判定為 `explicit = true`,但資料庫裡已有一列 `explicit = false`,執行 `UPDATE InstalledPackages SET explicit = 1 WHERE name = ?1`(升級規則)。

### 孤兒判斷:`crates/dpm/src/utils/orphan.rs`(新檔)

```rust
pub fn find_orphans(installed: &[DbPackage]) -> Vec<DbPackage>
```

1. 掃一次 `installed`,把每個套件 `dependencies` 欄位裡出現過的所有名字收進一個「被依賴」集合(`HashSet<String>`)。
2. 第一輪孤兒 = `installed` 中 `explicit == false` 且 `name` 不在「被依賴」集合裡的套件。
3. 重複步驟 1-2,但把已收集到的孤兒從 `installed` 的有效集合中剔除後重算「被依賴」集合——直到某一輪沒有新孤兒出現(fixpoint)。依賴圖是 DAG(pubgrub 求解本身保證無環),所以這個迴圈保證終止,上限是套件數量。

回傳值是 `DbPackage` 清單(順序:先被判定成孤兒的排前面),供 CLI 直接印名字或逐一移除。

### `Db::remove_installed_package`(新方法,同時修掉既有 SQL 注入風味的寫法)

`uninstall()` 目前用 `self.ctx.db.execute_query(&format!("DELETE FROM InstalledPackages WHERE name = '{}'", pkg))`——套件名稱直接字串插值進 SQL,沒有參數化。因為 `autoremove` 需要重用同一段「移除單一已裝套件的 DB 記錄」邏輯,順手抽成:

```rust
pub async fn remove_installed_package(&self, name: &str) -> ClientResult<()> {
    conn.execute("DELETE FROM InstalledPackages WHERE name = ?1", [name]).await
}
```

`uninstall()` 改呼叫這個方法而不是手組字串。移除單一套件的「檔案系統清理」部分(`get_installed_files`/`remove_installed_files`/刪 `Software/<pkg>`、`opt/<pkg>`、`bin/<pkg>`)抽成 `uninstall_package_files(&self, pkg: &str)` 私有 helper,`uninstall()`、`autoremove()` 都呼叫這個 + `remove_installed_package`。

### CLI 指令

`cli_parse.rs::Commands` 新增:

```rust
/// Remove orphaned dependencies (installed automatically, no longer needed)
#[command(visible_aliases = ["ar", "auto"])]
Autoremove {
    #[arg(short, long)]
    verbose: bool,
},
```

`action.rs::autoremove()`:

1. `self.ctx.db.read_all()` 拿全部已裝套件。
2. `find_orphans(&installed)`。
3. 空 → 印 `"No orphaned packages found."`。
4. 非空 → 印出每個孤兒的名字/版本,逐一呼叫共用的移除 helper(檔案系統 + DB 記錄),完成後印總結(清了幾個)。

`uninstall()` 收尾(現有迴圈跑完後)加一段:重新 `read_all()` + `find_orphans`,非空就印 `"{n} package(s) are now orphaned: {names}. Run 'dpm autoremove' to remove them."`,不動手清。

## 資料流

**安裝**:`dpm install foo`(foo 依賴 bar)→ `resolve_install_set` 解出 `[foo, bar]` → 寫入 `InstalledPackages`:`foo` 的 `(source,name)` 在 `requests` 裡 → `explicit=1`;`bar` 不在 → `explicit=0`。

**升級為 explicit**:使用者之後 `dpm install bar`(直接指名)→ `bar` 這次在 `requests` 裡 → 資料庫裡已有一列 `explicit=0` 的 `bar` → `UPDATE ... SET explicit=1`。

**孤兒產生 + 清理**:`dpm uninstall foo` → `foo` 那列被刪 → `bar` 不再被任何已裝套件的 `dependencies` 引用,且 `explicit=0` → `uninstall()` 收尾印出「`bar` 現在是孤兒」提示 → 使用者跑 `dpm autoremove` → `find_orphans` 抓到 `bar` → 清除。

## 錯誤處理

沿用現有 `ClientError`/`ClientResult` 模式,不新增 error variant:

- `ALTER TABLE` 的「欄位已存在」錯誤字串比對失敗(判斷邏輯本身出錯,不是欄位真的已存在)→ 正常回傳 `ClientError::Core(DatabaseError(...))`,跟其他 DB 操作一致。
- `autoremove`/`uninstall` 收尾的孤兒查詢失敗 → 沿用現有的 `?` 傳播,不特別吞掉(這兩處都不是「盡量做、失敗也不影響主流程」的場景——移除操作本身失敗必須讓使用者看到)。

## 測試計畫

- `db.rs`:`explicit` 欄位讀寫 round-trip;開啟一個「舊格式」(手動建表、不含 `explicit`)的 DB 檔案,確認 `run_migrations` 能補上欄位且不報錯,既有列讀回來 `explicit=true`;`remove_installed_package` 刪除正確的列、不影響其他列。
- `orphan.rs`:單層孤兒(A 依賴 B,A 被移除,B 變孤兒);多層遞迴孤兒(A 依賴 B 依賴 C,A 被移除,B 和 C 都變孤兒);`explicit=true` 的套件即使沒有任何東西依賴它也不算孤兒;仍被其他已裝套件依賴的 `explicit=false` 套件不算孤兒;空清單/無孤兒的情況。
- `action.rs`(比照現有 `sync_source_inner` 測試風格,用 `Context::for_test`):`install` 寫入 `explicit` 正確(直接指名 vs 依賴解出);已是 `explicit=false` 的套件被直接指名安裝後升級為 `true`;`autoremove` 端到端(裝 A+B,A 依賴 B,移除 A,跑 `autoremove`,確認 B 被清、`InstalledPackages`/`installed_files`/檔案系統都乾淨);`uninstall` 收尾的孤兒提示訊息正確列出孤兒名字。

## 驗證清單

- [ ] `cargo check --workspace`/`cargo clippy --workspace --all-targets -- -D warnings`/`cargo test --workspace` 通過
- [ ] 新單元測試涵蓋上述情境
- [ ] 手動驗證:裝一個有依賴的套件、移除主套件、確認 `uninstall` 印出孤兒提示、跑 `dpm autoremove` 確認依賴被清乾淨
- [ ] TODO.md 「功能缺口」的 autoremove 項目打勾
