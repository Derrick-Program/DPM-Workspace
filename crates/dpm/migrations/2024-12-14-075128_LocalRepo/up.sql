-- Your SQL goes here
CREATE TABLE IF NOT EXISTS LocalRepo (
    name            TEXT PRIMARY KEY NOT NULL UNIQUE,
    version         TEXT NOT NULL,
    url             TEXT NOT NULL,
    description     TEXT NOT NULL,
    filename        TEXT NOT NULL,
    hash            TEXT NOT NULL,
    entry           TEXT NOT NULL,
    dependencies    TEXT 
);