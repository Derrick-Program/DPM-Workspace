# DPM-Workspace 常用指令
# 用法: just <recipe>,列出全部: just --list

# 預設顯示可用指令
default:
    @just --list

# ── 開發 ────────────────────────────────────────────

# 快速檢查整個 workspace
check:
    cargo check --workspace

# 編譯 (debug)
build:
    cargo build --workspace

# 編譯 (release, 已開 lto + strip)
release:
    cargo build --workspace --release

# 跑全部測試
test:
    cargo test --workspace

# 跑指定 crate 的測試, 例: just test-p DPM
test-p crate:
    cargo test -p {{crate}}

# clippy (warning 視為錯誤)
lint:
    cargo clippy --workspace --all-targets -- -D warnings

# clippy 自動修
lint-fix:
    cargo clippy --workspace --all-targets --fix --allow-dirty

# 格式化
fmt:
    cargo fmt --all

# 檢查格式 (CI 用)
fmt-check:
    cargo fmt --all -- --check

# 提交前檢查: 格式 + clippy + 測試
pre-commit: fmt lint test

# ── 執行 ────────────────────────────────────────────

# 跑 client, 例: just run-client install foo
run-client *args:
    cargo run -p DPM -- {{args}}

# 跑 server, 例: just run-server init
run-server *args:
    cargo run -p DPM-Server -- {{args}}

# ── Diesel (client DB) ─────────────────────────────

# 新增 migration, 例: just migration-new add_column
migration-new name:
    cd crates/dpm && diesel migration generate {{name}}

# 套用 migrations (需 DATABASE_URL)
migration-run:
    cd crates/dpm && diesel migration run

# 重跑最後一個 migration
migration-redo:
    cd crates/dpm && diesel migration redo

# ── 文件與維護 ──────────────────────────────────────

# 產生並開啟文件
doc:
    cargo doc --workspace --no-deps --open

# 清除編譯產物
clean:
    cargo clean

# 檢查過期 dependency (需 cargo-outdated)
outdated:
    cargo outdated --workspace

# 檢查安全性漏洞 (需 cargo-audit)
audit:
    cargo audit

# 更新 Cargo.lock
update:
    cargo update

# ── 安裝 ────────────────────────────────────────────

# 安裝 dpm client 到 ~/.cargo/bin
install-client:
    cargo install --path crates/dpm

# 安裝 dpm-server 到 ~/.cargo/bin
install-server:
    cargo install --path crates/dpm-server
