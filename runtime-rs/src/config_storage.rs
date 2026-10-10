//! 配置文档共用原子写入，格式和状态仍由各领域负责。
use crate::ApiError;
use serde_json::Value;
use std::path::Path;

pub(crate) fn write_object(path: &Path, value: &Value) -> Result<(), ApiError> {
    let parent = path
        .parent()
        .ok_or_else(|| ApiError::internal("configuration parent missing"))?;
    std::fs::create_dir_all(parent).map_err(|e| ApiError::internal(e.to_string()))?;
    let mut entropy = [0_u8; 8];
    getrandom::getrandom(&mut entropy).map_err(|e| ApiError::internal(e.to_string()))?;
    let suffix: String = entropy.iter().map(|b| format!("{b:02x}")).collect();
    let temporary = parent.join(format!(".pisper-config-{suffix}.tmp"));
    let result = (|| -> Result<(), ApiError> {
        use std::io::Write;
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options
            .open(&temporary)
            .map_err(|e| ApiError::internal(e.to_string()))?;
        let bytes =
            serde_json::to_vec_pretty(value).map_err(|e| ApiError::internal(e.to_string()))?;
        file.write_all(&bytes)
            .and_then(|_| file.sync_all())
            .map_err(|e| ApiError::internal(e.to_string()))?;
        drop(file);
        // Windows 的 rename 同样支持替换文件，写入失败时旧文件仍完整保留。
        std::fs::rename(&temporary, path).map_err(|e| ApiError::internal(e.to_string()))?;
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    result
}
