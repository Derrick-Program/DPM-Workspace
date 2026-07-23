use super::{ClientError, ClientResult, DbPackage, NewDbPackage};
use diesel::{prelude::*, SqliteConnection};
use diesel_migrations::{embed_migrations, EmbeddedMigrations, MigrationHarness};
use dpm_core::CoreError::*;
use fs2::FileExt;
use futures_util::StreamExt;
use std::{
    fs::File,
    path::Path,
    sync::{Arc, Mutex},
};
use tokio::io::AsyncWriteExt;
pub const MIGRATIONS: EmbeddedMigrations = embed_migrations!();
pub struct Db {
    conn: Arc<Mutex<SqliteConnection>>,
    _lock_file: File,
}

impl Db {
    pub fn new(database_url: &str, lock_file_path: &str) -> ClientResult<Self> {
        let lock_file = File::create(lock_file_path).map_err(|e| {
            ClientError::LockError(format!("Failed to create lock file: {}", e))
        })?;
        lock_file.lock_exclusive().map_err(|e| {
            ClientError::LockError(format!("Failed to lock file: {}", e))
        })?;
        let conn = SqliteConnection::establish(database_url)
            .map_err(|e| ClientError::Core(DatabaseError(e.to_string())))?;
        Ok(Db {
            conn: Arc::new(Mutex::new(conn)),
            _lock_file: lock_file,
        })
    }

    pub fn run_migrations(&mut self) -> ClientResult<()> {
        let mut conn = self
            .conn
            .lock()
            .map_err(|e| ClientError::LockError(e.to_string()))?;

        conn.run_pending_migrations(MIGRATIONS)
            .map_err(|e| ClientError::Core(DatabaseError(e.to_string())))?;
        Ok(())
    }
    fn conn_operation<T, F>(&self, operation: F) -> ClientResult<T>
    where
        F: FnOnce(&mut SqliteConnection) -> ClientResult<T>,
    {
        let mut conn = self
            .conn
            .lock()
            .map_err(|e| ClientError::LockError(e.to_string()))?;
        operation(&mut conn)
    }
    pub fn execute_query(&self, query: &str) -> ClientResult<()> {
        self.conn_operation(|conn| {
            diesel::sql_query(query)
                .execute(conn)
                .map(|_| ())
                .map_err(|e| ClientError::Core(DatabaseError(e.to_string())))
        })
    }

    pub fn insert(&self, pkg: DbPackage) -> ClientResult<()> {
        use crate::schema::LocalRepo;
        let dependencies_json = pkg
            .dependencies
            .map(|deps| serde_json::to_string(&deps).unwrap_or_else(|_| "[]".to_string()))
            .unwrap_or_else(|| "[]".to_string());

        let insertable_pkg = NewDbPackage {
            name: pkg.name,
            version: pkg.version,
            url: pkg.url,
            description: pkg.description,
            filename: pkg.filename,
            hash: pkg.hash,
            entry: pkg.entry,
            dependencies: Some(dependencies_json),
        };

        self.conn_operation(|conn| {
            diesel::insert_into(LocalRepo::table)
                .values(&insertable_pkg)
                .execute(conn)
                .map(|_| ())
                .map_err(|e| ClientError::Core(DatabaseError(e.to_string())))
        })
    }

    pub fn read_all(&self) -> ClientResult<Vec<DbPackage>> {
        use crate::schema::LocalRepo::dsl::*;
        self.conn_operation(|conn| {
            // 加載所有資料
            let results = LocalRepo
                .load::<(
                    String,
                    String,
                    String,
                    String,
                    String,
                    String,
                    String,
                    Option<String>,
                )>(conn)
                .map_err(|e| ClientError::Core(DatabaseError(e.to_string())))?;
            let packages: Vec<DbPackage> = results
                .into_iter()
                .map(
                    |(
                        name1,
                        version1,
                        url1,
                        description1,
                        filename1,
                        hash1,
                        entry1,
                        dependencies_json,
                    )| {
                        let dependencies1 = dependencies_json
                            .as_deref() // 獲取 Option<String> 的切片
                            .and_then(|json| serde_json::from_str(json).ok()); // 嘗試反序列化

                        DbPackage {
                            name: name1,
                            version: version1,
                            url: url1,
                            description: description1,
                            filename: filename1,
                            hash: hash1,
                            entry: entry1,
                            dependencies: dependencies1,
                        }
                    },
                )
                .collect();

            Ok(packages)
        })
    }

    pub fn read_one(&self, target_name: &str) -> ClientResult<Option<DbPackage>> {
        use crate::schema::LocalRepo::dsl::*;
        self.conn_operation(|conn| {
            let result = LocalRepo
                .filter(name.eq(target_name))
                .first::<(
                    String,
                    String,
                    String,
                    String,
                    String,
                    String,
                    String,
                    Option<String>,
                )>(conn)
                .optional()
                .map_err(|e| ClientError::Core(DatabaseError(e.to_string())))?;

            if let Some((
                name1,
                version1,
                url1,
                description1,
                filename1,
                hash1,
                entry1,
                dependencies_json,
            )) = result
            {
                let dependencies1 = dependencies_json
                    .as_deref()
                    .and_then(|json| serde_json::from_str(json).ok());
                Ok(Some(DbPackage {
                    name: name1,
                    version: version1,
                    url: url1,
                    description: description1,
                    filename: filename1,
                    hash: hash1,
                    entry: entry1,
                    dependencies: dependencies1,
                }))
            } else {
                Ok(None)
            }
        })
    }

    pub fn update_version(&self, target_name: &str, new_version: &str) -> ClientResult<()> {
        use crate::schema::LocalRepo::dsl::*;
        self.conn_operation(|conn| {
            diesel::update(LocalRepo.filter(name.eq(target_name)))
                .set(version.eq(new_version))
                .execute(conn)
                .map(|_| ())
                .map_err(|e| ClientError::Core(DatabaseError(e.to_string())))
        })
    }

    pub fn delete(&self, target_name: &str) -> ClientResult<()> {
        use crate::schema::LocalRepo::dsl::*;
        self.conn_operation(|conn| {
            diesel::delete(LocalRepo.filter(name.eq(target_name)))
                .execute(conn)
                .map(|_| ())
                .map_err(|e| ClientError::Core(DatabaseError(e.to_string())))
        })
    }
    pub fn read_by_condition(
        &self,
        condition: Box<
            dyn BoxableExpression<
                crate::schema::LocalRepo::table,
                diesel::sqlite::Sqlite,
                SqlType = diesel::sql_types::Bool,
            >,
        >,
    ) -> ClientResult<Vec<DbPackage>> {
        use crate::schema::LocalRepo::dsl::*;

        self.conn_operation(|conn| {
            let results = LocalRepo
                .filter(condition)
                .load::<(
                    String,
                    String,
                    String,
                    String,
                    String,
                    String,
                    String,
                    Option<String>,
                )>(conn)
                .map_err(|e| ClientError::Core(DatabaseError(e.to_string())))?;
            let packages: Vec<DbPackage> = results
                .into_iter()
                .map(
                    |(
                        name1,
                        version1,
                        url1,
                        description1,
                        filename1,
                        hash1,
                        entry1,
                        dependencies_json,
                    )| {
                        let dependencies1 = dependencies_json
                            .as_deref()
                            .and_then(|json| serde_json::from_str(json).ok());

                        DbPackage {
                            name: name1,
                            version: version1,
                            url: url1,
                            description: description1,
                            filename: filename1,
                            hash: hash1,
                            entry: entry1,
                            dependencies: dependencies1,
                        }
                    },
                )
                .collect();

            Ok(packages)
        })
    }
    // read_by_condition example
    // let condition = Box::new(crate::schema::LocalRepo::dsl::name.eq("example_name".to_string()));

    // let result = db.read_by_condition(condition)?;

    // for package in result {
    //     println!("{:?}", package);
    // }
    pub async fn download_file(&self, name: &str) -> ClientResult<()> {
        let package = self
            .read_one(name)?
            .ok_or_else(|| ClientError::Core(PackageNotFound(name.to_string())))?;
        let url = &package.url;
        let filename = Path::new("/tmp").join(&package.filename);
        let req = reqwest::get(url)
            .await
            .map_err(|e| ClientError::Core(NetworkError(e.to_string())))?;
        if !req.status().is_success() {
            return Err(ClientError::Core(NetworkError(
                format!("Failed to download file: HTTP {}", req.status()),
            )));
        }
        let mut file = tokio::fs::File::create(&filename)
            .await
            .map_err(|e| ClientError::Core(IoError(e)))?;
        let mut stream = req.bytes_stream();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|e| {
                ClientError::SystemError(format!("Failed to read chunk: {}", e))
            })?;
            file.write_all(&chunk).await.map_err(|e| {
                ClientError::SystemError(format!("Failed to write chunk: {}", e))
            })?;
        }
        println!("File downloaded to: {}", filename.display());
        Ok(())
    }
    pub fn drop_table(&self, tname: &str) -> ClientResult<()> {
        self.execute_query(&format!("DROP TABLE IF EXISTS {}", tname))?;
        Ok(())
    }
    pub fn clear_table(&self, tname: &str) -> ClientResult<()> {
        self.execute_query(&format!("DELETE FROM {}", tname))?;
        Ok(())
    }
}
