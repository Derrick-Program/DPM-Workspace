#[cfg(test)]
mod tests {
    use dpm_core::*;
    use std::path::Path;

    mod versioning_tests {
        use dpm_core::*;

        fn prebuilt(version: &str) -> PackageVersionInfo {
            PackageVersionInfo {
                version: version.to_string(),
                kind: PackageKind::Prebuilt {
                    url: format!("http://example.com/{version}"),
                    hash: "hash123".to_string(),
                    file_name: "file1.zip".to_string(),
                },
                dependencies: None,
                entry: None,
                description: None,
            }
        }

        #[test]
        fn has_package_false_before_any_version_added() {
            let repo = RepoInfo::new();
            assert!(!repo.has_package("package1"));
        }

        #[test]
        fn versions_of_missing_package_is_not_found() {
            let repo = RepoInfo::new();
            let result = repo.versions_of("nonexistent");
            assert!(result.is_err());
        }

        #[cfg(feature = "server")]
        mod server_tests {
            use super::prebuilt;
            use dpm_core::*;

            #[test]
            fn add_package_version_appends_and_is_queryable() {
                let mut repo = RepoInfo::new();
                repo.add_package_version("package1".to_string(), prebuilt("1.0.0"))
                    .unwrap();

                assert!(repo.has_package("package1"));
                let versions = repo.versions_of("package1").unwrap();
                assert_eq!(versions.len(), 1);
                assert_eq!(versions[0].version, "1.0.0");
            }

            #[test]
            fn add_package_version_keeps_multiple_versions() {
                let mut repo = RepoInfo::new();
                repo.add_package_version("package1".to_string(), prebuilt("1.0.0"))
                    .unwrap();
                repo.add_package_version("package1".to_string(), prebuilt("2.0.0"))
                    .unwrap();

                let versions = repo.versions_of("package1").unwrap();
                assert_eq!(versions.len(), 2);
                assert_eq!(repo.latest_version("package1").unwrap().version, "2.0.0");
            }

            #[test]
            fn add_package_version_rejects_duplicate_version() {
                let mut repo = RepoInfo::new();
                repo.add_package_version("package1".to_string(), prebuilt("1.0.0"))
                    .unwrap();

                let result = repo.add_package_version("package1".to_string(), prebuilt("1.0.0"));
                assert!(
                    result.is_err(),
                    "publishing the same version twice must fail"
                );
            }

            #[test]
            fn remove_package_version_removes_only_that_version() {
                let mut repo = RepoInfo::new();
                repo.add_package_version("package1".to_string(), prebuilt("1.0.0"))
                    .unwrap();
                repo.add_package_version("package1".to_string(), prebuilt("2.0.0"))
                    .unwrap();

                let removed = repo.remove_package_version("package1", "1.0.0").unwrap();
                assert_eq!(removed.version, "1.0.0");
                let remaining = repo.versions_of("package1").unwrap();
                assert_eq!(remaining.len(), 1);
                assert_eq!(remaining[0].version, "2.0.0");
            }

            #[test]
            fn remove_last_package_version_drops_the_package_entirely() {
                let mut repo = RepoInfo::new();
                repo.add_package_version("package1".to_string(), prebuilt("1.0.0"))
                    .unwrap();

                repo.remove_package_version("package1", "1.0.0").unwrap();
                assert!(!repo.has_package("package1"));
            }

            #[test]
            fn remove_package_version_missing_version_errors() {
                let mut repo = RepoInfo::new();
                repo.add_package_version("package1".to_string(), prebuilt("1.0.0"))
                    .unwrap();

                let result = repo.remove_package_version("package1", "9.9.9");
                assert!(result.is_err());
            }
        }
    }

    // 測試 JsonStorage 功能

    #[test]
    fn test_from_json() {
        let test_file = "test.json";
        let content = r#"{
        "package_name": "test_package",
        "file_name": "test_file.zip",
        "version": "1.0.0",
        "description": "A test package",
        "hash": "hash123",
        "dependencies": null
    }"#;

        std::fs::write(test_file, content).unwrap();

        let package_info: PackageInfo = JsonStorage::from_json(Path::new(test_file)).unwrap();
        assert_eq!(package_info.package_name, "test_package");
        assert_eq!(package_info.version, "1.0.0");

        std::fs::remove_file(test_file).unwrap();
    }

    #[test]
    fn test_to_json() {
        let package_info = PackageInfo::new(
            "test_package".to_string(),
            "test_file.zip".to_string(),
            "1.0.0".to_string(),
            "A test package".to_string(),
            "hash123".to_string(),
            None,
        );

        let test_file = "output.json";
        JsonStorage::to_json(&package_info, Path::new(test_file)).unwrap();

        let written_content = std::fs::read_to_string(test_file).unwrap();
        assert!(written_content.contains("\"package_name\": \"test_package\""));

        std::fs::remove_file(test_file).unwrap();
    }

    #[tokio::test]
    async fn test_from_url() {
        let url = "https://jsonplaceholder.typicode.com/posts/1";
        let result: serde_json::Value = JsonStorage::from_url(url).await.unwrap();
        assert!(result.is_object());
        assert_eq!(result["id"], 1);
    }

    #[test]
    fn test_from_str_to() {
        let json_str = r#"{
        "package_name": "test_package",
        "file_name": "test_file.zip",
        "version": "1.0.0",
        "description": "A test package",
        "hash": "hash123",
        "dependencies": null
    }"#;

        let package_info: PackageInfo = JsonStorage::from_str_to(json_str).unwrap();
        assert_eq!(package_info.package_name, "test_package");
        assert_eq!(package_info.version, "1.0.0");
    }
    // 測試 Dependency 功能
    #[test]
    fn test_dependency_serde() {
        let dependency = Dependency {
            name: "serde".to_string(),
            version: "1.0.0".to_string(),
        };

        let serialized = serde_json::to_string(&dependency).unwrap();
        assert!(serialized.contains("\"name\":\"serde\""));

        let deserialized: Dependency = serde_json::from_str(&serialized).unwrap();
        assert_eq!(deserialized.name, "serde");
        assert_eq!(deserialized.version, "1.0.0");
    }

    #[test]
    fn test_hash_file_is_deterministic_and_content_sensitive() {
        let file_a = "hash_test_a.txt";
        let file_b = "hash_test_b.txt";
        std::fs::write(file_a, b"hello world").unwrap();
        std::fs::write(file_b, b"different content").unwrap();

        let hash_a1 = hash_file(Path::new(file_a)).unwrap();
        let hash_a2 = hash_file(Path::new(file_a)).unwrap();
        let hash_b = hash_file(Path::new(file_b)).unwrap();

        assert_eq!(
            hash_a1, hash_a2,
            "hashing the same file twice must be deterministic"
        );
        assert_ne!(hash_a1, hash_b, "different content must hash differently");
        assert_eq!(
            hash_a1.len(),
            64,
            "blake3 hex output is 32 bytes = 64 hex chars"
        );

        std::fs::remove_file(file_a).unwrap();
        std::fs::remove_file(file_b).unwrap();
    }
}
