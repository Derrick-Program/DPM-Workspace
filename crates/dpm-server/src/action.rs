use crate::*;
use colored::Colorize;
use dpm_core::zip_folder;
use dpm_core::CoreError;
use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::io::Write;
#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;
use walkdir::WalkDir;
pub fn keygen(obj: &Keygen, keys_dir: &Path) -> ServerResult<()> {
    dpm_core::validate_author_id(&obj.author_id)?;
    std::fs::create_dir_all(keys_dir)?;
    let priv_path = keys_dir.join(format!("{}.priv", obj.author_id));
    let pub_path = keys_dir.join(format!("{}.pub", obj.author_id));
    if !obj.force && (priv_path.exists() || pub_path.exists()) {
        return Err(ServerError::ValidationError(format!(
            "key for author '{}' already exists at {}; pass --force to overwrite",
            obj.author_id,
            keys_dir.display()
        )));
    }

    let signing_key = dpm_core::generate_signing_key()?;
    // 私鑰是機密——用 0600(僅擁有者可讀寫)寫入,不吃 umask 預設的 0644。
    let mut priv_opts = OpenOptions::new();
    priv_opts.write(true).create(true).truncate(true);
    #[cfg(unix)]
    priv_opts.mode(0o600);
    let mut priv_file = priv_opts.open(&priv_path)?;
    priv_file.write_all(&signing_key.to_bytes())?;
    std::fs::write(&pub_path, signing_key.verifying_key().to_bytes())?;

    // 私鑰絕對不能被 commit——即使資料 repo 自己的 .gitignore 忘了擋,
    // 這裡也自己確保 keys/ 底下有一條 *.priv 規則。
    let gitignore_path = keys_dir.join(".gitignore");
    let existing = std::fs::read_to_string(&gitignore_path).unwrap_or_default();
    if !existing.lines().any(|l| l.trim() == "*.priv") {
        let mut updated = existing;
        if !updated.is_empty() && !updated.ends_with('\n') {
            updated.push('\n');
        }
        updated.push_str("*.priv\n");
        std::fs::write(&gitignore_path, updated)?;
    }

    println!(
        "Generated key pair for '{}':\n  private: {} (do not commit)\n  public:  {} (commit this)",
        obj.author_id,
        priv_path.display(),
        pub_path.display()
    );
    Ok(())
}

pub fn hash(obj: &Hash, project_src: &Path, repo_dir: &Path) -> ServerResult<()> {
    let project_path = project_src.join(&obj.package_name);
    if !project_path.exists() {
        return Err(ServerError::Core(CoreError::PackageNotFound(
            obj.package_name.clone(),
        )));
    }
    let project_info = &project_path.join("packageInfo.json");

    let hash = if let Some(build_command) = &obj.build {
        // kind: source——沒有下載內容可雜湊,綁定 build_command 本身跟目前
        // git HEAD commit,讓 Source 套件也有東西可以簽、可以驗。
        let commit = source_repo_commit_hash(&project_path)?;
        dpm_core::hash_bytes(format!("{build_command}\n{commit}").as_bytes())
    } else {
        let zip_path = repo_dir.join(format!("{}.zip", obj.package_name));
        if zip_path.exists() {
            // kind: prebuilt,且 `dpm-server build` 已經跑過——直接雜湊那個
            // zip,讓「簽的 hash」等於「fix add 之後 client 會拿去驗證下載
            // 內容的 hash」,兩者是同一個值。
            dpm_core::hash_file(&zip_path)?
        } else {
            // 還沒 build(或者根本不是要發布的 prebuilt 套件)——退回舊行為:
            // 逐檔雜湊整個專案目錄寫進 hashes.json。
            let hashfile = &project_path.join("hashes.json");
            let mut hashes: HashMap<String, String> =
                JsonStorage::from_json(hashfile).unwrap_or_else(|_| HashMap::new());
            let mut counter: i32 = 0;
            for entry in WalkDir::new(&project_path) {
                let entry = entry?;
                let path = entry.path();
                if path.is_file() && path != hashfile {
                    counter += 1;
                    let file_hash = dpm_core::hash_file(path)?;
                    let relative_path = path.strip_prefix(&project_path).unwrap_or(path);
                    println!(
                        "{} {} {} {}",
                        counter,
                        relative_path.display().to_string().yellow(),
                        "===>".green(),
                        file_hash.bold().blue(),
                    );
                    hashes.insert(relative_path.display().to_string(), file_hash);
                }
            }
            JsonStorage::to_json(&hashes, hashfile)?;
            counter += 1;
            let hashes_json_hash = dpm_core::hash_file(hashfile)?;
            println!(
                "{} {} {} {}",
                counter,
                "hashes.json".yellow(),
                "===>".green(),
                hashes_json_hash.bold().blue(),
            );
            hashes.insert("hashes.json".to_string(), hashes_json_hash.clone());
            JsonStorage::to_json(&hashes, hashfile)?;
            hashes_json_hash
        }
    };

    let mut package_info: PackageInfo = JsonStorage::from_json(project_info)?;
    package_info.hash = hash;
    if let Some(cmd) = &obj.build {
        package_info.build_command = Some(cmd.clone());
    }
    JsonStorage::to_json(&package_info, project_info)?;
    Ok(())
}

/// 解析出包含 `project_path` 的 git repo 目前 HEAD 的 commit hash(從
/// `project_path` 往上找 `.git`,所以不管 `dpm-server` 是從 repo 根目錄還是
/// 子目錄執行都找得到)。用來把一個 `kind: source` 套件簽出來的 hash 綁定
/// 在「發布當下原始碼樹的確切狀態」——光是 `build_command` 字串本身不能防止
/// 有人在不改 build 指令的情況下換掉底下的原始碼。
fn source_repo_commit_hash(project_path: &Path) -> ServerResult<String> {
    let repo = git2::Repository::discover(project_path).map_err(|e| {
        ServerError::ValidationError(format!(
            "could not find a git repository containing {}: {e}",
            project_path.display()
        ))
    })?;
    let head = repo
        .head()
        .map_err(|e| ServerError::ValidationError(format!("could not resolve HEAD: {e}")))?;
    let commit = head
        .peel_to_commit()
        .map_err(|e| ServerError::ValidationError(format!("could not resolve HEAD commit: {e}")))?;
    Ok(commit.id().to_string())
}

pub fn sign(obj: &Sign, project_src: &Path, keys_dir: &Path) -> ServerResult<()> {
    let path = project_src.join(&obj.name);
    let info_path = path.join("packageInfo.json");
    let mut package_info: PackageInfo = JsonStorage::from_json(&info_path)?;
    let author = package_info.author.clone().ok_or_else(|| {
        ServerError::ValidationError(format!(
            "{} has no author recorded; run `dpm-server init --author <id>` first",
            obj.name
        ))
    })?;
    // Same path-traversal guard as `verify_publish_authorization`: `author`
    // comes straight from this package's packageInfo.json (the same
    // untrusted file that check distrusts) and is about to be used as a
    // path component below.
    dpm_core::validate_author_id(&author)?;

    let priv_path = keys_dir.join(format!("{author}.priv"));
    let priv_bytes = std::fs::read(&priv_path).map_err(|e| {
        ServerError::ValidationError(format!(
            "could not read private key for author '{author}' at {}: {e}",
            priv_path.display()
        ))
    })?;
    let signing_key = dpm_core::signing_key_from_bytes(&priv_bytes)?;

    let signature = dpm_core::sign_hash(&signing_key, &package_info.hash);
    package_info.signature = Some(signature);
    JsonStorage::to_json(&package_info, &info_path)?;
    println!("Signed {} (author: {author})", obj.name);
    Ok(())
}

pub fn build(obj: &Build, project_src: &Path, repo_dir: &Path) -> ServerResult<()> {
    let project_path = project_src.join(&obj.package_name);
    if !project_path.exists() {
        return Err(ServerError::Core(CoreError::PackageNotFound(
            obj.package_name.clone(),
        )));
    }
    let zip_file_path = repo_dir.join(format!("{}.zip", obj.package_name));
    zip_folder(&project_path, &zip_file_path)?;
    Ok(())
}

pub fn init(obj: &Init, project_src: &Path, keys_dir: &Path) -> ServerResult<()> {
    // `obj.author` ends up written verbatim into packageInfo.json, which
    // every other author-id call site (`verify_publish_authorization`,
    // `sign`, `dpm`'s `verify_official_signature`) treats as untrusted input
    // and uses as a path component — reject an invalid id here too, before
    // it's used as a path below or persisted downstream.
    dpm_core::validate_author_id(&obj.author)?;

    let pubkey_path = keys_dir.join(format!("{}.pub", obj.author));
    if !pubkey_path.exists() {
        return Err(ServerError::ValidationError(format!(
            "no public key found for author '{}' at {}; run `dpm-server keygen {}` first",
            obj.author,
            pubkey_path.display(),
            obj.author
        )));
    }

    let project_path = project_src.join(obj.name.as_str());
    if !project_path.exists() {
        create_dir_all(&project_path)?;
    } else {
        return Err(ServerError::ValidationError(format!(
            "{} already exists",
            project_path.display()
        )));
    }
    let entry_path = project_path.join(obj.entry.as_str());
    if let Some(parent) = entry_path.parent() {
        create_dir_all(parent)?;
    }
    File::create(&entry_path)?;
    let file_path = project_path.join("hashes.json");
    File::create(&file_path)?;
    let hash = dpm_core::hash_file(&file_path)?;
    let package_info = PackageInfo::new(
        obj.name.to_string(),
        obj.entry.to_string(),
        obj.ver.to_string(),
        obj.description.to_string(),
        hash,
        None,
        Some(obj.author.clone()),
    );
    JsonStorage::to_json(&package_info, &project_path.join("packageInfo.json"))?;
    Ok(())
}

pub async fn fix(
    obj: &Fix,
    conn: &turso::Connection,
    project_src: &Path,
    keys_dir: &Path,
) -> ServerResult<()> {
    match &obj.command {
        FixAction::Add(obj) => fix_add(obj, conn, project_src, keys_dir).await?,
        FixAction::Del(obj) => fix_del(obj, conn).await?,
    }
    Ok(())
}

/// `fix_add` 寫進 `RepoInfo.db` 之前的守門檢查,兩種 kind 共用:
/// 1. `packageInfo.json` 一定要有 `author`/`signature`/`hash`。
/// 2. `signature` 必須是 `author` 的公鑰對 `hash` 的合法簽章。
/// 3. 如果這個套件名稱在 `repo` 裡已經有版本,新版本的 `author` 必須跟第一次
///    發布時登記的 author 相同——這是防冒名頂替的核心檢查。沒有既有版本代表
///    這是第一次發布,直接放行(沒有「跟誰比對」的問題)。
async fn verify_publish_authorization(
    pk_info: &PackageInfo,
    conn: &turso::Connection,
    project_name: &str,
    keys_dir: &Path,
) -> ServerResult<()> {
    let author = pk_info.author.as_deref().ok_or_else(|| {
        ServerError::ValidationError(format!(
            "{project_name}'s packageInfo.json has no author; run `dpm-server init --author <id>`"
        ))
    })?;
    let signature = pk_info.signature.as_deref().ok_or_else(|| {
        ServerError::ValidationError(format!(
            "{project_name}'s packageInfo.json has no signature; run `dpm-server sign {project_name}` first"
        ))
    })?;

    // `author` comes straight from an attacker-controlled packageInfo.json in
    // a publish PR — it must not be usable as a path component. `Path::join`
    // discards the base entirely on an absolute string, and `..` segments
    // escape `keys_dir` either way, which would let a malicious author id
    // point the "public key" read at an arbitrary file on disk.
    dpm_core::validate_author_id(author)?;

    let pubkey_path = keys_dir.join(format!("{author}.pub"));
    let pubkey_bytes = std::fs::read(&pubkey_path).map_err(|e| {
        ServerError::ValidationError(format!(
            "could not read public key for author '{author}' at {}: {e}",
            pubkey_path.display()
        ))
    })?;
    let verifying_key = dpm_core::verifying_key_from_bytes(&pubkey_bytes).map_err(|e| {
        ServerError::ValidationError(format!("invalid public key for author '{author}': {e}"))
    })?;
    dpm_core::verify_hash_signature(&verifying_key, &pk_info.hash, signature).map_err(|e| {
        ServerError::ValidationError(format!(
            "signature verification failed for {project_name}: {e}"
        ))
    })?;

    let mut rows = conn
        .query(
            "SELECT author FROM Packages WHERE name = ?1 ORDER BY version ASC LIMIT 1",
            [project_name],
        )
        .await
        .map_err(|e| ServerError::ValidationError(format!("Database error: {e}")))?;
    let existing_author: Option<String> = if let Some(row) = rows
        .next()
        .await
        .map_err(|e| ServerError::ValidationError(format!("Database error: {e}")))?
    {
        row.get_value(0)
            .ok()
            .and_then(|v| v.as_text().map(|s| s.to_string()))
    } else {
        None
    };

    if let Some(existing) = existing_author {
        if existing != author {
            return Err(ServerError::ValidationError(format!(
                "{project_name} was first published by author '{existing}', but this version is signed by '{author}' — authorship cannot change without manual review"
            )));
        }
    }
    Ok(())
}

async fn fix_add(
    obj: &Add,
    conn: &turso::Connection,
    project_src: &Path,
    keys_dir: &Path,
) -> ServerResult<()> {
    let path = project_src.join(&obj.project_name);
    let pk_info: PackageInfo = JsonStorage::from_json(&path.join("packageInfo.json"))?;

    verify_publish_authorization(&pk_info, conn, &obj.project_name, keys_dir).await?;

    let kind = match &obj.kind {
        AddKind::Url {
            url,
            file_name,
            target,
        } => {
            let is_local = url.starts_with("file://") || Path::new(url).exists();
            if !url.starts_with("https://") && !is_local {
                return Err(ServerError::ValidationError(format!(
                    "url {url} must use https://, file:// or be a valid local file path"
                )));
            }
            let file_name = file_name
                .clone()
                .or_else(|| url.rsplit('/').next().map(|s| s.to_string()))
                .filter(|s| !s.is_empty())
                .ok_or_else(|| {
                    ServerError::ValidationError(
                        "could not derive a file name from the url; pass --file-name explicitly"
                            .to_string(),
                    )
                })?;

            let bytes = if let Some(path_str) = url.strip_prefix("file://") {
                std::fs::read(path_str)?
            } else if Path::new(url).exists() {
                std::fs::read(url)?
            } else {
                let response = reqwest::get(url)
                    .await
                    .map_err(|e| ServerError::Core(CoreError::NetworkError(e.to_string())))?;
                if !response.status().is_success() {
                    return Err(ServerError::Core(CoreError::NetworkError(format!(
                        "failed to fetch {url}: HTTP {}",
                        response.status()
                    ))));
                }
                response
                    .bytes()
                    .await
                    .map_err(|e| ServerError::Core(CoreError::NetworkError(e.to_string())))?
                    .to_vec()
            };

            let tmp_path = std::env::temp_dir().join(&file_name);
            std::fs::write(&tmp_path, &bytes)?;
            let downloaded_hash = dpm_core::hash_file(&tmp_path)?;
            std::fs::remove_file(&tmp_path)?;

            if downloaded_hash != pk_info.hash {
                return Err(ServerError::ValidationError(format!(
                    "content served at {url} (hash {downloaded_hash}) does not match {}'s signed hash ({}) — run `dpm-server build`, `hash`, and `sign` again after the url's content changes",
                    obj.project_name, pk_info.hash
                )));
            }

            PackageKind::Prebuilt {
                builds: vec![dpm_core::PrebuiltBuild {
                    target: target.clone(),
                    url: url.clone(),
                    hash: pk_info.hash.clone(),
                    file_name,
                }],
            }
        }
        AddKind::Build { build, targets } => {
            let effective_build = match build {
                Some(cmd) if !cmd.trim().is_empty() => cmd.clone(),
                _ => pk_info.build_command.clone().ok_or_else(|| {
                    ServerError::ValidationError(
                        "build command not specified and not found in packageInfo.json — pass build command or run 'dpm-server hash <name> --build \"<cmd>\"' first".to_string()
                    )
                })?,
            };

            let commit = source_repo_commit_hash(&path)?;
            let recomputed_hash =
                dpm_core::hash_bytes(format!("{effective_build}\n{commit}").as_bytes());
            if recomputed_hash != pk_info.hash {
                return Err(ServerError::ValidationError(format!(
                    "build command {effective_build:?} (hash {recomputed_hash}) does not match {}'s signed hash ({}) — run `dpm-server hash --build`/`sign` again after the build command changes",
                    obj.project_name, pk_info.hash
                )));
            }

            PackageKind::Source {
                build: effective_build,
                hash: Some(pk_info.hash.clone()),
                supported_targets: targets.clone(),
            }
        }
    };

    let (kind_str, url, filename, build_command, targets_str) = match &kind {
        PackageKind::Prebuilt { builds } => {
            let build = &builds[0];
            (
                "prebuilt",
                Some(build.url.clone()),
                Some(build.file_name.clone()),
                None,
                build.target.clone(),
            )
        }
        PackageKind::Source {
            build,
            supported_targets,
            ..
        } => (
            "source",
            None,
            None,
            Some(build.clone()),
            supported_targets.clone().map(|t| t.join(",")),
        ),
    };

    let dependencies_str =
        serde_json::to_string(&pk_info.dependencies).unwrap_or_else(|_| "{}".to_string());

    let to_value = |opt: Option<String>| match opt {
        Some(s) => turso::Value::Text(s),
        None => turso::Value::Null,
    };

    conn.execute(
        "INSERT INTO Packages (name, version, kind, url, hash, filename, build_command, description, entry, dependencies, author, signature, targets)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)
         ON CONFLICT(name, version) DO UPDATE SET
            kind = excluded.kind,
            url = excluded.url,
            hash = excluded.hash,
            filename = excluded.filename,
            build_command = excluded.build_command,
            description = excluded.description,
            entry = excluded.entry,
            dependencies = excluded.dependencies,
            author = excluded.author,
            signature = excluded.signature,
            targets = excluded.targets",
        vec![
            turso::Value::Text(obj.project_name.clone()),
            turso::Value::Text(pk_info.version.clone()),
            turso::Value::Text(kind_str.to_string()),
            to_value(url),
            to_value(Some(pk_info.hash)),
            to_value(filename),
            to_value(build_command),
            turso::Value::Text(pk_info.description),
            turso::Value::Text("".to_string()),
            to_value(Some(dependencies_str)),
            to_value(pk_info.author),
            to_value(pk_info.signature),
            to_value(targets_str),
        ],
    )
    .await
    .map_err(|e| ServerError::ValidationError(format!("Database insert error: {e}")))?;
    Ok(())
}
async fn fix_del(obj: &Del, conn: &turso::Connection) -> ServerResult<()> {
    let version = match &obj.version {
        Some(v) => v.clone(),
        None => {
            let mut rows = conn
                .query(
                    "SELECT version FROM Packages WHERE name = ?1 ORDER BY version ASC",
                    [obj.project_name.as_str()],
                )
                .await
                .map_err(|e| ServerError::ValidationError(format!("Database error: {e}")))?;
            let mut versions: Vec<String> = Vec::new();
            while let Some(row) = rows
                .next()
                .await
                .map_err(|e| ServerError::ValidationError(format!("Database error: {e}")))?
            {
                if let Some(v) = row
                    .get_value(0)
                    .ok()
                    .and_then(|val| val.as_text().map(|s| s.to_string()))
                {
                    versions.push(v);
                }
            }
            if versions.is_empty() {
                return Err(ServerError::Core(CoreError::PackageNotFound(
                    obj.project_name.clone(),
                )));
            }
            if versions.len() > 1 {
                return Err(ServerError::ValidationError(format!(
                    "package {} has {} published versions — specify which one to remove",
                    obj.project_name,
                    versions.len()
                )));
            }
            versions[0].clone()
        }
    };
    conn.execute(
        "DELETE FROM Packages WHERE name = ?1 AND version = ?2",
        [obj.project_name.clone(), version.clone()],
    )
    .await
    .map_err(|e| ServerError::ValidationError(format!("Database delete error: {e}")))?;
    println!(
        "Package '{}@{}' removed successfully.",
        obj.project_name, version
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 產生金鑰、跑 `init --author`+`hash`+`sign`,留下一份完整簽好名的
    /// `packageInfo.json`——Task 7 之後 `fix_add` 一律先驗證作者/簽章,
    /// 所有 `fix_add` 測試都需要這組前置。
    fn init_hash_sign(project_src: &Path, keys_dir: &Path, name: &str, author: &str) {
        keygen(
            &Keygen {
                author_id: author.to_string(),
                force: false,
            },
            keys_dir,
        )
        .unwrap();
        init(
            &Init {
                name: name.to_string(),
                entry: "main.sh".to_string(),
                ver: "0.1.0".to_string(),
                description: "a demo package".to_string(),
                author: author.to_string(),
            },
            project_src,
            keys_dir,
        )
        .unwrap();
        hash(
            &Hash {
                package_name: name.to_string(),
                build: None,
            },
            project_src,
            &project_src.join("unused-repo-dir"),
        )
        .unwrap();
        sign(
            &Sign {
                name: name.to_string(),
            },
            project_src,
            keys_dir,
        )
        .unwrap();
    }

    /// Same as `init_hash_sign`, but for `AddKind::Build` callers: signs the
    /// build-command hash (`source_repo_commit_hash` + `build_cmd`) instead
    /// of the file-walk hash, so it matches what `fix_add`'s `Build` arm
    /// recomputes and compares against. Requires `project_src` itself to be
    /// a git repo (see `init_git_repo`).
    fn init_hash_sign_for_build(
        project_src: &Path,
        keys_dir: &Path,
        name: &str,
        author: &str,
        build_cmd: &str,
    ) {
        keygen(
            &Keygen {
                author_id: author.to_string(),
                force: false,
            },
            keys_dir,
        )
        .unwrap();
        init(
            &Init {
                name: name.to_string(),
                entry: "main.sh".to_string(),
                ver: "0.1.0".to_string(),
                description: "a demo package".to_string(),
                author: author.to_string(),
            },
            project_src,
            keys_dir,
        )
        .unwrap();
        hash(
            &Hash {
                package_name: name.to_string(),
                build: Some(build_cmd.to_string()),
            },
            project_src,
            &project_src.join("unused-repo-dir"),
        )
        .unwrap();
        sign(
            &Sign {
                name: name.to_string(),
            },
            project_src,
            keys_dir,
        )
        .unwrap();
    }

    async fn create_test_db(_project_src: &Path) -> (tempfile::TempDir, turso::Connection) {
        let temp_dir = tempfile::tempdir().unwrap();
        let db_path = temp_dir.path().join("RepoInfo.db");
        let db = turso::Builder::new_local(db_path.to_str().unwrap())
            .build()
            .await
            .unwrap();
        let conn = db.connect().unwrap();
        conn.execute(
            "CREATE TABLE IF NOT EXISTS Packages (
                name TEXT NOT NULL,
                version TEXT NOT NULL,
                kind TEXT NOT NULL,
                url TEXT,
                hash TEXT,
                filename TEXT,
                build_command TEXT,
                description TEXT NOT NULL,
                entry TEXT,
                dependencies TEXT,
                author TEXT,
                signature TEXT,
                targets TEXT,
                PRIMARY KEY (name, version)
            )",
            (),
        )
        .await
        .unwrap();
        (temp_dir, conn)
    }

    #[tokio::test]
    async fn fix_add_rejects_a_second_version_signed_by_a_different_author() {
        let project_src = std::env::temp_dir().join(format!(
            "dpm-server-fix-add-author-mismatch-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&project_src).unwrap();
        let keys_dir = project_src.join("keys");

        init_git_repo(&project_src);
        init_hash_sign_for_build(&project_src, &keys_dir, "demo-pkg", "alice", "v1 build");
        let (_td, conn) = create_test_db(&project_src).await;

        fix_add(
            &Add {
                project_name: "demo-pkg".to_string(),
                kind: AddKind::Build {
                    build: Some("v1 build".to_string()),
                    targets: None,
                },
            },
            &conn,
            &project_src,
            &keys_dir,
        )
        .await
        .unwrap();

        keygen(
            &Keygen {
                author_id: "mallory".to_string(),
                force: false,
            },
            &keys_dir,
        )
        .unwrap();
        let info_path = project_src.join("demo-pkg").join("packageInfo.json");
        let mut package_info: PackageInfo = JsonStorage::from_json(&info_path).unwrap();
        package_info.version = "0.2.0".to_string();
        package_info.author = Some("mallory".to_string());
        package_info.signature = None;
        JsonStorage::to_json(&package_info, &info_path).unwrap();
        hash(
            &Hash {
                package_name: "demo-pkg".to_string(),
                build: None,
            },
            &project_src,
            &project_src.join("unused-repo-dir"),
        )
        .unwrap();
        sign(
            &Sign {
                name: "demo-pkg".to_string(),
            },
            &project_src,
            &keys_dir,
        )
        .unwrap();

        let err = fix_add(
            &Add {
                project_name: "demo-pkg".to_string(),
                kind: AddKind::Build {
                    build: Some("v2 build".to_string()),
                    targets: None,
                },
            },
            &conn,
            &project_src,
            &keys_dir,
        )
        .await
        .unwrap_err();
        assert!(matches!(err, ServerError::ValidationError(_)));
        let mut rows = conn
            .query("SELECT count(*) FROM Packages WHERE name = 'demo-pkg'", ())
            .await
            .unwrap();
        let count: i64 = *rows
            .next()
            .await
            .unwrap()
            .unwrap()
            .get_value(0)
            .unwrap()
            .as_integer()
            .unwrap();
        assert_eq!(count, 1, "the rejected v2 must not be added");

        std::fs::remove_dir_all(&project_src).ok();
    }

    #[tokio::test]
    async fn fix_add_accepts_a_second_version_from_the_same_author() {
        let project_src = std::env::temp_dir().join(format!(
            "dpm-server-fix-add-same-author-second-version-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&project_src).unwrap();
        let keys_dir = project_src.join("keys");

        init_git_repo(&project_src);
        init_hash_sign_for_build(&project_src, &keys_dir, "demo-pkg", "alice", "v1 build");
        let (_td, conn) = create_test_db(&project_src).await;

        fix_add(
            &Add {
                project_name: "demo-pkg".to_string(),
                kind: AddKind::Build {
                    build: Some("v1 build".to_string()),
                    targets: None,
                },
            },
            &conn,
            &project_src,
            &keys_dir,
        )
        .await
        .unwrap();

        let info_path = project_src.join("demo-pkg").join("packageInfo.json");
        let mut package_info: PackageInfo = JsonStorage::from_json(&info_path).unwrap();
        package_info.version = "0.2.0".to_string();
        package_info.signature = None;
        JsonStorage::to_json(&package_info, &info_path).unwrap();
        hash(
            &Hash {
                package_name: "demo-pkg".to_string(),
                build: Some("v2 build".to_string()),
            },
            &project_src,
            &project_src.join("unused-repo-dir"),
        )
        .unwrap();
        sign(
            &Sign {
                name: "demo-pkg".to_string(),
            },
            &project_src,
            &keys_dir,
        )
        .unwrap();

        fix_add(
            &Add {
                project_name: "demo-pkg".to_string(),
                kind: AddKind::Build {
                    build: Some("v2 build".to_string()),
                    targets: None,
                },
            },
            &conn,
            &project_src,
            &keys_dir,
        )
        .await
        .unwrap();

        let mut rows = conn
            .query("SELECT count(*) FROM Packages WHERE name = 'demo-pkg'", ())
            .await
            .unwrap();
        let count: i64 = *rows
            .next()
            .await
            .unwrap()
            .unwrap()
            .get_value(0)
            .unwrap()
            .as_integer()
            .unwrap();
        assert_eq!(count, 2);

        let mut rows = conn
            .query(
                "SELECT author FROM Packages WHERE name = 'demo-pkg' ORDER BY version DESC LIMIT 1",
                (),
            )
            .await
            .unwrap();
        let latest_author: String = rows
            .next()
            .await
            .unwrap()
            .unwrap()
            .get_value(0)
            .unwrap()
            .as_text()
            .unwrap()
            .to_string();
        assert_eq!(latest_author, "alice");

        std::fs::remove_dir_all(&project_src).ok();
    }

    #[tokio::test]
    async fn fix_add_rejects_a_tampered_signature() {
        let project_src = std::env::temp_dir().join(format!(
            "dpm-server-fix-add-bad-sig-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&project_src).unwrap();
        let keys_dir = project_src.join("keys");
        init_hash_sign(&project_src, &keys_dir, "demo-pkg", "alice");

        let info_path = project_src.join("demo-pkg").join("packageInfo.json");
        let mut package_info: PackageInfo = JsonStorage::from_json(&info_path).unwrap();
        package_info.signature = Some("0".repeat(128));
        JsonStorage::to_json(&package_info, &info_path).unwrap();

        let (_td, conn) = create_test_db(&project_src).await;
        let err = fix_add(
            &Add {
                project_name: "demo-pkg".to_string(),
                kind: AddKind::Build {
                    build: Some("cargo build".to_string()),
                    targets: None,
                },
            },
            &conn,
            &project_src,
            &keys_dir,
        )
        .await
        .unwrap_err();
        assert!(matches!(err, ServerError::ValidationError(_)));
        let mut rows = conn
            .query("SELECT count(*) FROM Packages WHERE name = 'demo-pkg'", ())
            .await
            .unwrap();
        let count: i64 = *rows
            .next()
            .await
            .unwrap()
            .unwrap()
            .get_value(0)
            .unwrap()
            .as_integer()
            .unwrap();
        assert_eq!(count, 0);

        std::fs::remove_dir_all(&project_src).ok();
    }

    /// `project_src` used to come from a process-wide `OnceLock`, so nothing
    /// in this file could be unit tested without a real global. Now that
    /// it's a plain parameter, `init()` can run against an isolated tempdir
    /// — this is the check that init() actually respects that parameter
    /// instead of falling back to some ambient path.
    #[test]
    fn init_creates_package_skeleton_under_given_project_src() {
        let project_src = std::env::temp_dir().join(format!(
            "dpm-server-action-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&project_src).unwrap();

        let keys_dir = project_src.join("keys");
        keygen(
            &Keygen {
                author_id: "alice".to_string(),
                force: false,
            },
            &keys_dir,
        )
        .unwrap();

        let obj = Init {
            name: "demo-pkg".to_string(),
            entry: "main.sh".to_string(),
            ver: "0.1.0".to_string(),
            description: "a demo package".to_string(),
            author: "alice".to_string(),
        };

        init(&obj, &project_src, &keys_dir).unwrap();

        let pkg_dir = project_src.join("demo-pkg");
        assert!(pkg_dir.join("main.sh").exists());
        assert!(pkg_dir.join("hashes.json").exists());
        assert!(pkg_dir.join("packageInfo.json").exists());

        std::fs::remove_dir_all(&project_src).ok();
    }

    /// `build()` used to read `current_dir()` directly for its output path
    /// even after `project_src` was parameterized — the one function in
    /// this file the round-1 fix didn't reach. Now `repo_dir` is a plain
    /// parameter too, so this runs against isolated tempdirs instead of
    /// depending on (and littering) the test runner's real working directory.
    #[test]
    fn build_zips_package_into_given_repo_dir() {
        let root = std::env::temp_dir().join(format!(
            "dpm-server-action-build-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let project_src = root.join("packages");
        let repo_dir = root.join("Repo");
        std::fs::create_dir_all(&project_src).unwrap();
        std::fs::create_dir_all(&repo_dir).unwrap();

        let keys_dir = project_src.join("keys");
        keygen(
            &Keygen {
                author_id: "alice".to_string(),
                force: false,
            },
            &keys_dir,
        )
        .unwrap();

        init(
            &Init {
                name: "demo-pkg".to_string(),
                entry: "main.sh".to_string(),
                ver: "0.1.0".to_string(),
                description: "a demo package".to_string(),
                author: "alice".to_string(),
            },
            &project_src,
            &keys_dir,
        )
        .unwrap();

        build(
            &Build {
                package_name: "demo-pkg".to_string(),
            },
            &project_src,
            &repo_dir,
        )
        .unwrap();

        assert!(repo_dir.join("demo-pkg.zip").exists());

        std::fs::remove_dir_all(&root).ok();
    }

    /// `hash()` used to write `hashes.json` then immediately re-read it back
    /// with a `.unwrap_or_else(|_| HashMap::new())` fallback — a redundant
    /// round-trip that, on any read failure, would silently discard every
    /// hash the walk just computed. The fix removes the re-read entirely;
    /// this test pins down that the entry file's hash actually ends up in
    /// `hashes.json` and that `packageInfo.json.hash` gets updated to the
    /// hash of `hashes.json` itself.
    #[test]
    fn hash_records_entry_file_hash_and_updates_package_info() {
        let project_src = std::env::temp_dir().join(format!(
            "dpm-server-action-hash-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&project_src).unwrap();

        let keys_dir = project_src.join("keys");
        keygen(
            &Keygen {
                author_id: "alice".to_string(),
                force: false,
            },
            &keys_dir,
        )
        .unwrap();

        init(
            &Init {
                name: "demo-pkg".to_string(),
                entry: "main.sh".to_string(),
                ver: "0.1.0".to_string(),
                description: "a demo package".to_string(),
                author: "alice".to_string(),
            },
            &project_src,
            &keys_dir,
        )
        .unwrap();

        hash(
            &Hash {
                package_name: "demo-pkg".to_string(),
                build: None,
            },
            &project_src,
            &project_src.join("unused-repo-dir"),
        )
        .unwrap();

        let pkg_dir = project_src.join("demo-pkg");
        let hashes: HashMap<String, String> =
            JsonStorage::from_json(&pkg_dir.join("hashes.json")).unwrap();
        // This is the actual regression guard: the redundant re-read this
        // fix removed had a `.unwrap_or_else(|_| HashMap::new())` fallback,
        // so if it ever hit a read error, every walked entry below would
        // have been silently discarded and only the self-hash entry would
        // survive. If that regressed, these two keys would be missing.
        assert!(
            hashes.contains_key("main.sh"),
            "hashes.json must still record the entry file's hash"
        );
        assert!(
            hashes.contains_key("packageInfo.json"),
            "hashes.json must still record packageInfo.json's hash"
        );
        assert!(
            hashes.contains_key("hashes.json"),
            "hashes.json must record its own self-hash entry"
        );

        // Not asserting packageInfo.json.hash against a reconstructed
        // first-pass file here: `HashMap`'s iteration order is randomized
        // per-process, so re-serializing an equivalent map in the test
        // produces different (but equally valid) JSON byte-for-byte,
        // which would make a byte-hash comparison flaky across process
        // runs for reasons unrelated to this fix. A shape check is the
        // stable thing to assert instead.
        let package_info: PackageInfo =
            JsonStorage::from_json(&pkg_dir.join("packageInfo.json")).unwrap();
        assert_eq!(
            package_info.hash.len(),
            64,
            "packageInfo.json.hash must be a full blake3 hex digest, not empty/truncated"
        );

        std::fs::remove_dir_all(&project_src).ok();
    }

    /// Regression guard for the `Add`/`AddKind` clap-enum refactor: the
    /// `Build` variant must reach `PackageKind::Source` without touching the
    /// network, and the resulting `RepoInfo` entry must carry the build
    /// command through untouched.
    #[tokio::test]
    async fn fix_add_build_variant_records_a_source_kind_package() {
        let project_src = std::env::temp_dir().join(format!(
            "dpm-server-action-fix-add-build-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&project_src).unwrap();
        let keys_dir = project_src.join("keys");
        init_git_repo(&project_src);
        init_hash_sign_for_build(
            &project_src,
            &keys_dir,
            "demo-pkg",
            "alice",
            "cargo build --release",
        );

        let (_td, conn) = create_test_db(&project_src).await;
        let add = Add {
            project_name: "demo-pkg".to_string(),
            kind: AddKind::Build {
                build: Some("cargo build --release".to_string()),
                targets: None,
            },
        };
        fix_add(&add, &conn, &project_src, &keys_dir).await.unwrap();

        let mut rows = conn
            .query(
                "SELECT author FROM Packages WHERE name = 'demo-pkg' ORDER BY version DESC LIMIT 1",
                (),
            )
            .await
            .unwrap();
        let latest_author: String = rows
            .next()
            .await
            .unwrap()
            .unwrap()
            .get_value(0)
            .unwrap()
            .as_text()
            .unwrap()
            .to_string();
        assert_eq!(latest_author, "alice");

        let mut rows = conn
            .query("SELECT kind, build_command, hash FROM Packages WHERE name = 'demo-pkg' ORDER BY version DESC LIMIT 1", ())
            .await
            .unwrap();
        let row = rows.next().await.unwrap().unwrap();
        let kind: String = row.get_value(0).unwrap().as_text().unwrap().to_string();
        let build_command: Option<String> = row
            .get_value(1)
            .ok()
            .and_then(|v| v.as_text().map(|s| s.to_string()));
        let hash: Option<String> = row
            .get_value(2)
            .ok()
            .and_then(|v| v.as_text().map(|s| s.to_string()));
        assert_eq!(kind, "source");
        assert_eq!(build_command.unwrap(), "cargo build --release");
        assert!(hash.is_some());

        std::fs::remove_dir_all(&project_src).ok();
    }

    #[tokio::test]
    async fn fix_add_build_variant_defaults_to_package_info_build_command_when_none() {
        let project_src = std::env::temp_dir().join(format!(
            "dpm-server-fix-add-build-fallback-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&project_src).unwrap();
        let keys_dir = project_src.join("keys");
        init_git_repo(&project_src);
        init_hash_sign_for_build(
            &project_src,
            &keys_dir,
            "demo-pkg",
            "alice",
            "cargo build --release",
        );

        let (_td, conn) = create_test_db(&project_src).await;
        let add = Add {
            project_name: "demo-pkg".to_string(),
            kind: AddKind::Build {
                build: None,
                targets: None,
            },
        };
        fix_add(&add, &conn, &project_src, &keys_dir).await.unwrap();

        let mut rows = conn
            .query(
                "SELECT kind, build_command FROM Packages WHERE name = 'demo-pkg' LIMIT 1",
                (),
            )
            .await
            .unwrap();
        let row = rows.next().await.unwrap().unwrap();
        let kind: String = row.get_value(0).unwrap().as_text().unwrap().to_string();
        let build_command: String = row.get_value(1).unwrap().as_text().unwrap().to_string();
        assert_eq!(kind, "source");
        assert_eq!(build_command, "cargo build --release");

        std::fs::remove_dir_all(&project_src).ok();
    }

    #[tokio::test]
    async fn fix_add_rejects_a_build_command_that_does_not_match_the_signed_hash() {
        let project_src = std::env::temp_dir().join(format!(
            "dpm-server-fix-add-build-hash-mismatch-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&project_src).unwrap();
        let keys_dir = project_src.join("keys");
        init_git_repo(&project_src);
        init_hash_sign_for_build(
            &project_src,
            &keys_dir,
            "demo-pkg",
            "alice",
            "cargo build --release",
        );

        let (_td, conn) = create_test_db(&project_src).await;
        let err = fix_add(
            &Add {
                project_name: "demo-pkg".to_string(),
                kind: AddKind::Build {
                    build: Some("cargo build --release --features malicious".to_string()),
                    targets: None,
                },
            },
            &conn,
            &project_src,
            &keys_dir,
        )
        .await
        .unwrap_err();
        assert!(matches!(err, ServerError::ValidationError(_)));
        let mut rows = conn
            .query("SELECT count(*) FROM Packages WHERE name = 'demo-pkg'", ())
            .await
            .unwrap();
        let count: i64 = *rows
            .next()
            .await
            .unwrap()
            .unwrap()
            .get_value(0)
            .unwrap()
            .as_integer()
            .unwrap();
        assert_eq!(count, 0);

        std::fs::remove_dir_all(&project_src).ok();
    }

    #[tokio::test]
    async fn fix_add_url_variant_rejects_non_https_before_any_network_call() {
        let project_src = std::env::temp_dir().join(format!(
            "dpm-server-action-fix-add-url-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&project_src).unwrap();
        let keys_dir = project_src.join("keys");
        init_hash_sign(&project_src, &keys_dir, "demo-pkg", "alice");

        let (_td, conn) = create_test_db(&project_src).await;
        let add = Add {
            project_name: "demo-pkg".to_string(),
            kind: AddKind::Url {
                url: "http://example.com/pkg.zip".to_string(),
                file_name: None,
                target: None,
            },
        };
        let err = fix_add(&add, &conn, &project_src, &keys_dir)
            .await
            .unwrap_err();
        assert!(matches!(err, ServerError::ValidationError(_)));
        let mut rows = conn
            .query("SELECT count(*) FROM Packages WHERE name = 'demo-pkg'", ())
            .await
            .unwrap();
        let count: i64 = *rows
            .next()
            .await
            .unwrap()
            .unwrap()
            .get_value(0)
            .unwrap()
            .as_integer()
            .unwrap();
        assert_eq!(
            count, 0,
            "a rejected url must not leave a partial entry in RepoInfo"
        );

        std::fs::remove_dir_all(&project_src).ok();
    }

    #[test]
    fn init_rejects_missing_author_key() {
        let project_src = std::env::temp_dir().join(format!(
            "dpm-server-init-no-key-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&project_src).unwrap();
        let keys_dir = project_src.join("keys");

        let err = init(
            &Init {
                name: "demo-pkg".to_string(),
                entry: "main.sh".to_string(),
                ver: "0.1.0".to_string(),
                description: "a demo package".to_string(),
                author: "nobody".to_string(),
            },
            &project_src,
            &keys_dir,
        )
        .unwrap_err();
        assert!(matches!(err, ServerError::ValidationError(_)));
        assert!(
            !project_src.join("demo-pkg").exists(),
            "must not create the package skeleton without a key"
        );

        std::fs::remove_dir_all(&project_src).ok();
    }

    /// Important 2: `init()` used to be one of two call sites that built a
    /// path out of the CLI-provided `author` (`keys_dir.join(format!("{}.pub",
    /// obj.author))`) with no charset validation — unlike
    /// `verify_publish_authorization`, which already rejected a
    /// path-traversal author id. Now both funnel through
    /// `dpm_core::validate_author_id`; this pins down that `init()` rejects
    /// a malicious author id before it touches any path or gets persisted
    /// into `packageInfo.json`.
    #[test]
    fn init_rejects_a_path_traversal_author_id() {
        let project_src = std::env::temp_dir().join(format!(
            "dpm-server-init-path-traversal-author-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&project_src).unwrap();
        let keys_dir = project_src.join("keys");

        let err = init(
            &Init {
                name: "demo-pkg".to_string(),
                entry: "main.sh".to_string(),
                ver: "0.1.0".to_string(),
                description: "a demo package".to_string(),
                author: "../../../../mallory/evil-keys/main/keys/mallory".to_string(),
            },
            &project_src,
            &keys_dir,
        )
        .unwrap_err();
        assert!(matches!(
            err,
            ServerError::Core(CoreError::SignatureInvalid(_))
        ));
        assert!(
            !project_src.join("demo-pkg").exists(),
            "must not create the package skeleton for a rejected author id"
        );

        std::fs::remove_dir_all(&project_src).ok();
    }

    #[test]
    fn keygen_rejects_a_path_traversal_author_id() {
        let keys_dir = std::env::temp_dir().join(format!(
            "dpm-server-keygen-path-traversal-author-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));

        let err = keygen(
            &Keygen {
                author_id: "../../../../mallory/evil-keys/main/keys/mallory".to_string(),
                force: false,
            },
            &keys_dir,
        )
        .unwrap_err();
        assert!(matches!(
            err,
            ServerError::Core(CoreError::SignatureInvalid(_))
        ));
        assert!(
            !keys_dir.exists(),
            "must not create keys_dir for a rejected author id"
        );
    }

    #[test]
    fn init_records_author_in_package_info() {
        let project_src = std::env::temp_dir().join(format!(
            "dpm-server-init-author-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&project_src).unwrap();
        let keys_dir = project_src.join("keys");
        keygen(
            &Keygen {
                author_id: "alice".to_string(),
                force: false,
            },
            &keys_dir,
        )
        .unwrap();

        init(
            &Init {
                name: "demo-pkg".to_string(),
                entry: "main.sh".to_string(),
                ver: "0.1.0".to_string(),
                description: "a demo package".to_string(),
                author: "alice".to_string(),
            },
            &project_src,
            &keys_dir,
        )
        .unwrap();

        let package_info: PackageInfo =
            JsonStorage::from_json(&project_src.join("demo-pkg").join("packageInfo.json")).unwrap();
        assert_eq!(package_info.author.as_deref(), Some("alice"));
        assert_eq!(package_info.signature, None);

        std::fs::remove_dir_all(&project_src).ok();
    }

    #[test]
    fn keygen_produces_32_byte_raw_key_files_and_a_gitignore() {
        let keys_dir = std::env::temp_dir().join(format!(
            "dpm-server-keygen-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&keys_dir).unwrap();

        keygen(
            &Keygen {
                author_id: "alice".to_string(),
                force: false,
            },
            &keys_dir,
        )
        .unwrap();

        let priv_bytes = std::fs::read(keys_dir.join("alice.priv")).unwrap();
        let pub_bytes = std::fs::read(keys_dir.join("alice.pub")).unwrap();
        assert_eq!(priv_bytes.len(), 32);
        assert_eq!(pub_bytes.len(), 32);

        let gitignore = std::fs::read_to_string(keys_dir.join(".gitignore")).unwrap();
        assert!(gitignore.lines().any(|l| l.trim() == "*.priv"));

        std::fs::remove_dir_all(&keys_dir).ok();
    }

    #[test]
    fn keygen_refuses_to_overwrite_without_force() {
        let keys_dir = std::env::temp_dir().join(format!(
            "dpm-server-keygen-overwrite-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&keys_dir).unwrap();

        keygen(
            &Keygen {
                author_id: "alice".to_string(),
                force: false,
            },
            &keys_dir,
        )
        .unwrap();
        let err = keygen(
            &Keygen {
                author_id: "alice".to_string(),
                force: false,
            },
            &keys_dir,
        )
        .unwrap_err();
        assert!(matches!(err, ServerError::ValidationError(_)));

        keygen(
            &Keygen {
                author_id: "alice".to_string(),
                force: true,
            },
            &keys_dir,
        )
        .unwrap();

        std::fs::remove_dir_all(&keys_dir).ok();
    }

    /// The private key is a real secret — whoever can read it can sign
    /// packages as this author. `fs::write`'s default umask-based mode
    /// (typically 0644) leaves it group/world-readable, so `keygen()` opens
    /// it with an explicit 0600 mode instead. This pins that down against
    /// the actual file, not just "did keygen not error".
    #[test]
    #[cfg(unix)]
    fn keygen_writes_private_key_with_0600_permissions() {
        use std::os::unix::fs::PermissionsExt;

        let keys_dir = std::env::temp_dir().join(format!(
            "dpm-server-keygen-perms-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&keys_dir).unwrap();

        keygen(
            &Keygen {
                author_id: "alice".to_string(),
                force: false,
            },
            &keys_dir,
        )
        .unwrap();

        let mode = std::fs::metadata(keys_dir.join("alice.priv"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o600);

        std::fs::remove_dir_all(&keys_dir).ok();
    }

    /// 在 `project_src` 這個目錄本身初始化一個 git repo 並 commit 一次
    /// (`--build` 模式的 `source_repo_commit_hash` 需要能在 `project_src`
    /// 底下找到 `.git`)。
    fn init_git_repo(project_src: &std::path::Path) {
        use git2::{Repository, Signature};
        let repo = Repository::init(project_src).unwrap();
        let mut index = repo.index().unwrap();
        index
            .add_all(["*"].iter(), git2::IndexAddOption::DEFAULT, None)
            .unwrap();
        index.write().unwrap();
        let tree_id = index.write_tree().unwrap();
        let tree = repo.find_tree(tree_id).unwrap();
        let sig = Signature::now("test", "test@example.com").unwrap();
        repo.commit(Some("HEAD"), &sig, &sig, "initial commit", &tree, &[])
            .unwrap();
    }

    #[test]
    fn hash_with_build_flag_hashes_build_command_plus_commit() {
        let project_src = std::env::temp_dir().join(format!(
            "dpm-server-hash-build-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&project_src).unwrap();
        let keys_dir = project_src.join("keys");
        keygen(
            &Keygen {
                author_id: "alice".to_string(),
                force: false,
            },
            &keys_dir,
        )
        .unwrap();
        init(
            &Init {
                name: "demo-pkg".to_string(),
                entry: "main.sh".to_string(),
                ver: "0.1.0".to_string(),
                description: "a demo package".to_string(),
                author: "alice".to_string(),
            },
            &project_src,
            &keys_dir,
        )
        .unwrap();
        init_git_repo(&project_src);

        let repo_dir = project_src.join("unused-repo-dir");
        hash(
            &Hash {
                package_name: "demo-pkg".to_string(),
                build: Some("cargo build --release".to_string()),
            },
            &project_src,
            &repo_dir,
        )
        .unwrap();

        let package_info: PackageInfo =
            JsonStorage::from_json(&project_src.join("demo-pkg").join("packageInfo.json")).unwrap();
        assert_eq!(
            package_info.hash.len(),
            64,
            "must be a full blake3 hex digest"
        );

        // 同樣的 build_command,重跑一次必須得到一樣的 hash(HEAD 沒變)。
        hash(
            &Hash {
                package_name: "demo-pkg".to_string(),
                build: Some("cargo build --release".to_string()),
            },
            &project_src,
            &repo_dir,
        )
        .unwrap();
        let package_info_again: PackageInfo =
            JsonStorage::from_json(&project_src.join("demo-pkg").join("packageInfo.json")).unwrap();
        assert_eq!(package_info.hash, package_info_again.hash);

        // 換一個 build_command,hash 必須不同。
        hash(
            &Hash {
                package_name: "demo-pkg".to_string(),
                build: Some("cargo build".to_string()),
            },
            &project_src,
            &repo_dir,
        )
        .unwrap();
        let package_info_different: PackageInfo =
            JsonStorage::from_json(&project_src.join("demo-pkg").join("packageInfo.json")).unwrap();
        assert_ne!(package_info.hash, package_info_different.hash);

        std::fs::remove_dir_all(&project_src).ok();
    }

    #[test]
    fn hash_uses_the_zip_file_directly_when_it_already_exists() {
        let root = std::env::temp_dir().join(format!(
            "dpm-server-hash-zip-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let project_src = root.join("packages");
        let repo_dir = root.join("Repo");
        std::fs::create_dir_all(&project_src).unwrap();
        std::fs::create_dir_all(&repo_dir).unwrap();
        let keys_dir = root.join("keys");
        keygen(
            &Keygen {
                author_id: "alice".to_string(),
                force: false,
            },
            &keys_dir,
        )
        .unwrap();
        init(
            &Init {
                name: "demo-pkg".to_string(),
                entry: "main.sh".to_string(),
                ver: "0.1.0".to_string(),
                description: "a demo package".to_string(),
                author: "alice".to_string(),
            },
            &project_src,
            &keys_dir,
        )
        .unwrap();
        build(
            &Build {
                package_name: "demo-pkg".to_string(),
            },
            &project_src,
            &repo_dir,
        )
        .unwrap();
        let expected_hash = dpm_core::hash_file(&repo_dir.join("demo-pkg.zip")).unwrap();

        hash(
            &Hash {
                package_name: "demo-pkg".to_string(),
                build: None,
            },
            &project_src,
            &repo_dir,
        )
        .unwrap();

        let package_info: PackageInfo =
            JsonStorage::from_json(&project_src.join("demo-pkg").join("packageInfo.json")).unwrap();
        assert_eq!(package_info.hash, expected_hash);

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn sign_writes_a_verifiable_signature() {
        let project_src = std::env::temp_dir().join(format!(
            "dpm-server-sign-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&project_src).unwrap();
        let keys_dir = project_src.join("keys");
        keygen(
            &Keygen {
                author_id: "alice".to_string(),
                force: false,
            },
            &keys_dir,
        )
        .unwrap();
        init(
            &Init {
                name: "demo-pkg".to_string(),
                entry: "main.sh".to_string(),
                ver: "0.1.0".to_string(),
                description: "a demo package".to_string(),
                author: "alice".to_string(),
            },
            &project_src,
            &keys_dir,
        )
        .unwrap();
        hash(
            &Hash {
                package_name: "demo-pkg".to_string(),
                build: None,
            },
            &project_src,
            &project_src.join("unused-repo-dir"),
        )
        .unwrap();

        sign(
            &Sign {
                name: "demo-pkg".to_string(),
            },
            &project_src,
            &keys_dir,
        )
        .unwrap();

        let package_info: PackageInfo =
            JsonStorage::from_json(&project_src.join("demo-pkg").join("packageInfo.json")).unwrap();
        let signature = package_info.signature.expect("sign must set a signature");

        let pubkey_bytes = std::fs::read(keys_dir.join("alice.pub")).unwrap();
        let verifying_key = dpm_core::verifying_key_from_bytes(&pubkey_bytes).unwrap();
        assert!(
            dpm_core::verify_hash_signature(&verifying_key, &package_info.hash, &signature)
                .is_ok(),
            "the written signature must verify against the package's own hash and author's public key"
        );

        std::fs::remove_dir_all(&project_src).ok();
    }

    #[test]
    fn sign_rejects_a_package_with_no_recorded_author() {
        // 直接手刻一個沒有 author 的 packageInfo.json,模擬 init 之前手動
        // 亂改檔案的狀況(舊格式,或不小心刪掉了 author 欄位)。
        let project_src = std::env::temp_dir().join(format!(
            "dpm-server-sign-no-author-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let pkg_dir = project_src.join("demo-pkg");
        std::fs::create_dir_all(&pkg_dir).unwrap();
        let package_info = PackageInfo::new(
            "demo-pkg".to_string(),
            "main.sh".to_string(),
            "0.1.0".to_string(),
            "a demo package".to_string(),
            "0".repeat(64),
            None,
            None,
        );
        JsonStorage::to_json(&package_info, &pkg_dir.join("packageInfo.json")).unwrap();

        let keys_dir = project_src.join("keys");
        let err = sign(
            &Sign {
                name: "demo-pkg".to_string(),
            },
            &project_src,
            &keys_dir,
        )
        .unwrap_err();
        assert!(matches!(err, ServerError::ValidationError(_)));

        std::fs::remove_dir_all(&project_src).ok();
    }

    #[test]
    fn add_kind_url_with_target_builds_a_prebuilt_kind_with_one_build_entry() {
        let target = Some("aarch64-apple-darwin".to_string());
        let kind = PackageKind::Prebuilt {
            builds: vec![dpm_core::PrebuiltBuild {
                target: target.clone(),
                url: "https://example.com/mac.zip".to_string(),
                hash: "a".repeat(64),
                file_name: "mac.zip".to_string(),
            }],
        };
        match kind {
            PackageKind::Prebuilt { builds } => {
                assert_eq!(builds.len(), 1);
                assert_eq!(builds[0].target, target);
            }
            _ => panic!("expected Prebuilt"),
        }
    }
}
