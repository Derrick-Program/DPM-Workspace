use DPM::{ClientError, ClientResult, Scope};

#[tokio::main]
async fn main() -> ClientResult<()> {
    DPM::init_cli_metadata();
    let args = DPM::get_args().map_err(|e| ClientError::SystemError(e.to_string()))?;
    let scope = if args.System {
        Scope::System
    } else {
        Scope::PerUser
    };
    if scope == Scope::System && cfg!(target_os = "linux") {
        // 不是 root 時會自動重新以 sudo 執行自己;已是 root 則直接繼續
        sudo::escalate_if_needed().map_err(|e| ClientError::SystemError(e.to_string()))?;
    }
    // scope 確定後才初始化全域路徑與資料庫
    DPM::set_globle_var(scope).await?;
    if let Err(e) = DPM::entry(args).await {
        eprintln!("{}", e);
        std::process::exit(1);
    }
    Ok(())
}
