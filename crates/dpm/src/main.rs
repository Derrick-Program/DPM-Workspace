use DPM::{set_globle_var, ClientError, ClientResult};

#[tokio::main]
async fn main() -> ClientResult<()> {
    if cfg!(target_os = "linux") {
        // 不是 root 時會自動重新以 sudo 執行自己;已是 root 則直接繼續
        sudo::escalate_if_needed().map_err(|e| ClientError::SystemError(e.to_string()))?;
    }
    // 權限確定後才初始化全域變數與資料庫（會碰 /opt/DPM）
    set_globle_var().await?;
    let args = DPM::get_args().map_err(|e| ClientError::SystemError(e.to_string()))?;
    if let Err(e) = DPM::entry(args).await {
        eprintln!("{}", e);
        std::process::exit(1);
    }
    Ok(())
}
