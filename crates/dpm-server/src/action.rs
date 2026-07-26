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
    let commit = head.peel_to_commit().map_err(|e| {
        ServerError::ValidationError(format!("could not resolve HEAD commit: {e}"))
    })?;
    Ok(commit.id().to_string())
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
    File::create(project_path.join(obj.entry.as_str()))?;
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

pub fn fix(obj: &Fix, repo: &mut RepoInfo, project_src: &Path) -> ServerResult<()> {
    match &obj.command {
        FixAction::Add(obj) => fix_add(obj, repo, project_src)?,
        FixAction::Del(obj) => fix_del(obj, repo)?,
    }
    Ok(())
}

fn fix_add(obj: &Add, repo: &mut RepoInfo, project_src: &Path) -> ServerResult<()> {
    let path = project_src.join(&obj.project_name);
    let pk_info: PackageInfo = JsonStorage::from_json(&path.join("packageInfo.json"))?;

    let kind = match &obj.kind {
        AddKind::Url { url, file_name } => {
            if !url.starts_with("https://") {
                return Err(ServerError::ValidationError(format!(
                    "url {url} must use https://"
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

            let response = reqwest::blocking::get(url)?;
            if !response.status().is_success() {
                return Err(ServerError::Core(CoreError::NetworkError(format!(
                    "failed to fetch {url}: HTTP {}",
                    response.status()
                ))));
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
        AddKind::Build { build } => PackageKind::Source {
            build: build.clone(),
            hash: None,
        },
    };

    let version_info = PackageVersionInfo {
        version: pk_info.version.clone(),
        kind,
        dependencies: pk_info.dependencies,
        entry: None,
        description: Some(pk_info.description),
        author: None,
        signature: None,
    };
    repo.add_package_version(obj.project_name.clone(), version_info)?;
    Ok(())
}
fn fix_del(obj: &Del, repo: &mut RepoInfo) -> ServerResult<()> {
    let version = match &obj.version {
        Some(v) => v.clone(),
        None => {
            let versions = repo.versions_of(&obj.project_name)?;
            if versions.len() > 1 {
                return Err(ServerError::ValidationError(format!(
                    "package {} has {} published versions — specify which one to remove",
                    obj.project_name,
                    versions.len()
                )));
            }
            versions
                .first()
                .ok_or_else(|| {
                    ServerError::Core(CoreError::PackageNotFound(obj.project_name.clone()))
                })?
                .version
                .clone()
        }
    };
    repo.remove_package_version(&obj.project_name, &version)?;
    println!(
        "Package '{}@{}' removed successfully.",
        obj.project_name, version
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

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
    #[test]
    fn fix_add_build_variant_records_a_source_kind_package() {
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

        let mut repo = RepoInfo::new();
        let add = Add {
            project_name: "demo-pkg".to_string(),
            kind: AddKind::Build {
                build: "cargo build --release".to_string(),
            },
        };
        fix_add(&add, &mut repo, &project_src).unwrap();

        let version_info = repo.latest_version("demo-pkg").unwrap();
        assert_eq!(version_info.version, "0.1.0");
        match &version_info.kind {
            PackageKind::Source { build, .. } => {
                assert_eq!(build, "cargo build --release");
            }
            other => panic!("expected PackageKind::Source, got {other:?}"),
        }

        std::fs::remove_dir_all(&project_src).ok();
    }

    /// The old `url`/`build` CLI flags used `conflicts_with` to reject
    /// "both" at parse time but still needed a runtime check for other
    /// per-variant validation (like the `https://` requirement) inside
    /// `fix_add` itself. `AddKind::Url` doesn't remove that check — it
    /// removes the "neither"/"both" states entirely, this test pins down
    /// that the remaining per-variant validation still runs and fails
    /// before any network call is attempted.
    #[test]
    fn fix_add_url_variant_rejects_non_https_before_any_network_call() {
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

        let mut repo = RepoInfo::new();
        let add = Add {
            project_name: "demo-pkg".to_string(),
            kind: AddKind::Url {
                url: "http://example.com/pkg.zip".to_string(),
                file_name: None,
            },
        };
        let err = fix_add(&add, &mut repo, &project_src).unwrap_err();
        assert!(matches!(err, ServerError::ValidationError(_)));
        assert!(
            repo.versions_of("demo-pkg").is_err(),
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
            JsonStorage::from_json(&project_src.join("demo-pkg").join("packageInfo.json"))
                .unwrap();
        assert_eq!(package_info.hash.len(), 64, "must be a full blake3 hex digest");

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
            JsonStorage::from_json(&project_src.join("demo-pkg").join("packageInfo.json"))
                .unwrap();
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
            JsonStorage::from_json(&project_src.join("demo-pkg").join("packageInfo.json"))
                .unwrap();
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
            JsonStorage::from_json(&project_src.join("demo-pkg").join("packageInfo.json"))
                .unwrap();
        assert_eq!(package_info.hash, expected_hash);

        std::fs::remove_dir_all(&root).ok();
    }
}
