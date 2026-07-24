# DPM-Workspace 常用指令
# 用法: just <recipe>,列出全部: just --list
# Secret 一律透過 Infisical 注入,執行前需先 `just env-login` + `just env-init`(見下方 Secrets 區塊)。
# 預設用 dev environment,可用 DPM_ENV=staging just <recipe> 覆寫。

env := env_var_or_default("DPM_ENV", "dev")
export MACOSX_DEPLOYMENT_TARGET := if os() == "macos" { "13.4" } else { "" }
export RUSTFLAGS := if os() == "macos" { "-C link-arg=-Wl,-no_fixup_chains" } else { "" }

# 預設顯示可用指令
default:
    @just --list

# ── 開發 ────────────────────────────────────────────

# 快速檢查整個 workspace
check:
    infisical run --env={{env}} --path=/ --command="cargo check --workspace"

# 編譯 (debug)
build:
    infisical run --env={{env}} --path=/ --command="cargo build --workspace"

# 編譯 (release, 已開 lto + strip)
release:
    infisical run --env={{env}} --path=/ --command="cargo build --workspace --release"

# 跑全部測試
test:
    infisical run --env={{env}} --path=/ --command="cargo test --workspace"

# 跑指定 crate 的測試, 例: just test-p DPM
test-p crate:
    infisical run --env={{env}} --path=/ --command="cargo test -p {{crate}}"

# clippy (warning 視為錯誤)
lint:
    infisical run --env={{env}} --path=/ --command="cargo clippy --workspace --all-targets -- -D warnings"

# clippy 自動修
lint-fix:
    infisical run --env={{env}} --path=/ --command="cargo clippy --workspace --all-targets --fix --allow-dirty"

# 格式化
fmt:
    infisical run --env={{env}} --path=/ --command="cargo fmt --all"

# 檢查格式 (CI 用)
fmt-check:
    infisical run --env={{env}} --path=/ --command="cargo fmt --all -- --check"

# 提交前檢查: 格式 + clippy + 測試
pre-commit: fmt lint test

# ── 執行 ────────────────────────────────────────────

# 跑 client, 例: just run-client install foo
run-client *args:
    infisical run --env={{env}} --path=/ --command="cargo run -p DPM -- {{args}}"

# 跑 server, 例: just run-server init
run-server *args:
    infisical run --env={{env}} --path=/ --command="cargo run -p DPM-Server -- {{args}}"

# ── 文件與維護 ──────────────────────────────────────

# 產生並開啟文件
doc:
    infisical run --env={{env}} --path=/ --command="cargo doc --workspace --no-deps --open"

# 清除編譯產物
clean:
    infisical run --env={{env}} --path=/ --command="cargo clean"

# 檢查過期 dependency (需 cargo-outdated)
outdated:
    infisical run --env={{env}} --path=/ --command="cargo outdated --workspace"

# 檢查安全性漏洞 (需 cargo-audit)
audit:
    infisical run --env={{env}} --path=/ --command="cargo audit"

# 更新 Cargo.lock
update:
    infisical run --env={{env}} --path=/ --command="cargo update"

# ── 安裝 ────────────────────────────────────────────

# 安裝 dpm client 到 ~/.cargo/bin
install-client:
    infisical run --env={{env}} --path=/ --command="cargo install --path crates/dpm"

# 安裝 dpm-server 到 ~/.cargo/bin
install-server:
    infisical run --env={{env}} --path=/ --command="cargo install --path crates/dpm-server"

# ── Secrets (Infisical) ─────────────────────────────

# 互動登入 Infisical(每台機器一次)
env-login:
    infisical login

# 建立/連結 .infisical.json 到既有 Infisical project(每個 repo 一次)
env-init:
    infisical init

# 列出目前 environment 的 secret(不印值)
env-list:
    infisical secrets --env={{env}} --path=/

# 批次匯入既有 dotenv 檔案內容到指定 environment, 例: just env-push path/to/.env dev
env-push dotenv target_env:
    infisical secrets set --file="{{dotenv}}" --env={{target_env}}
