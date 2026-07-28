CREATE TABLE IF NOT EXISTS installed_files (
    package_name TEXT NOT NULL,
    file_path TEXT NOT NULL,
    PRIMARY KEY (package_name, file_path)
);
