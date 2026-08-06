use std::net::TcpListener;
use DPM::{ActionInfo, Context, Setting, Source};

fn serve_bytes_once(bytes: Vec<u8>) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();

    std::thread::spawn(move || {
        if let Ok((mut stream, _)) = listener.accept() {
            use std::io::{Read, Write};
            let mut buf = [0u8; 1024];
            let _ = stream.read(&mut buf);
            let header = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                bytes.len()
            );
            let _ = stream.write_all(header.as_bytes());
            let _ = stream.write_all(&bytes);
            let _ = stream.flush();
        }
    });

    format!("http://127.0.0.1:{port}")
}

async fn build_network_repo_db(pkg_name: &str, version: &str, desc: &str) -> Vec<u8> {
    let temp_dir = tempfile::tempdir().unwrap();
    let db_path = temp_dir.path().join("RepoInfo.db");
    {
        let conn = turso::Builder::new_local(db_path.to_str().unwrap())
            .build()
            .await
            .unwrap()
            .connect()
            .unwrap();

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

        conn.execute(
            "INSERT INTO Packages (name, version, kind, url, hash, filename, build_command, description, entry, dependencies, author, signature, targets)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
            vec![
                turso::Value::Text(pkg_name.to_string()),
                turso::Value::Text(version.to_string()),
                turso::Value::Text("prebuilt".to_string()),
                turso::Value::Text("https://example.com/demo.zip".to_string()),
                turso::Value::Text("a".repeat(64)),
                turso::Value::Text("demo.zip".to_string()),
                turso::Value::Null,
                turso::Value::Text(desc.to_string()),
                turso::Value::Text("".to_string()),
                turso::Value::Text("[]".to_string()),
                turso::Value::Null,
                turso::Value::Null,
                turso::Value::Null,
            ],
        )
        .await
        .unwrap();

        let _ = conn.execute("PRAGMA wal_checkpoint(FULL)", ()).await;
    }
    std::fs::read(&db_path).unwrap()
}

#[tokio::test]
async fn test_full_network_update_and_cache_query() {
    let db_bytes = build_network_repo_db("network-app", "1.0.0", "Fast network package").await;
    let server_url = serve_bytes_once(db_bytes);

    let root = tempfile::tempdir().unwrap();
    let ctx = Context::for_test(root.path()).await.unwrap();

    let source = Source {
        alias: "network-source".to_string(),
        repo_url: "https://example.com/repo".to_string(),
        repo_info: server_url,
    };

    let setting = Setting {
        sources: vec![source],
    };

    let action = ActionInfo::new(ctx.clone(), vec![], false, setting);

    // 1. Sync remote RepoInfo.db over network HTTP into LocalRepoInfo.db
    let update_res = action.update().await;
    assert!(
        update_res.is_ok(),
        "HTTP network update of RepoInfo.db must succeed"
    );

    // 2. Read LocalRepoInfo.db available packages
    let available = ctx.info_db.read_available().await.unwrap();
    assert_eq!(available.len(), 1);
    assert_eq!(available[0].name, "network-app");
    assert_eq!(available[0].version, "1.0.0");
    assert_eq!(available[0].description, "Fast network package");

    // 3. Test missing package prompt for an un-cached package
    let missing_action = ActionInfo::new(
        ctx.clone(),
        vec!["ghost-package".to_string()],
        false,
        Setting::default(),
    );
    let install_res = missing_action.install().await;
    assert!(install_res.is_err());
    let err_msg = install_res.unwrap_err().to_string();
    assert!(
        err_msg.contains("ghost-package") && err_msg.contains("dpm update"),
        "Missing package error must prompt user to run dpm update"
    );
}
