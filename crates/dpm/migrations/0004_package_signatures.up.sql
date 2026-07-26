DROP TABLE IF EXISTS LocalRepo;
CREATE TABLE IF NOT EXISTS LocalRepo (
    source TEXT NOT NULL,
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
    PRIMARY KEY (source, name, version)
);
