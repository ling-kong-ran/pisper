//! 配置文档共用原子写入；领域格式和迁移由调用方负责。
use crate::ApiError;
use serde_json::Value;
use std::path::Path;

pub(crate) fn write_object(path: &Path, value: &Value) -> Result<(), ApiError> {
    persist(path, value, true).map(|_| ())
}
pub(crate) fn create_object(path: &Path, value: &Value) -> Result<bool, ApiError> {
    persist(path, value, false)
}

fn persist(path: &Path, value: &Value, replace: bool) -> Result<bool, ApiError> {
    let parent = path
        .parent()
        .ok_or_else(|| ApiError::internal("configuration parent missing"))?;
    std::fs::create_dir_all(parent).map_err(|e| ApiError::internal(e.to_string()))?;
    let mut entropy = [0_u8; 8];
    getrandom::getrandom(&mut entropy).map_err(|e| ApiError::internal(e.to_string()))?;
    let suffix: String = entropy.iter().map(|b| format!("{b:02x}")).collect();
    let temporary = parent.join(format!(".pisper-config-{suffix}.tmp"));
    let result = (|| -> Result<bool, ApiError> {
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
        if replace {
            std::fs::rename(&temporary, path).map_err(|e| ApiError::internal(e.to_string()))?;
        } else {
            // 完整文件一次发布；即使另一实例同时创建目标，也不能覆盖它的配置。
            match std::fs::hard_link(&temporary, path) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                    return Ok(false)
                }
                Err(error) => return Err(ApiError::internal(error.to_string())),
            }
        }
        Ok(true)
    })();
    match std::fs::remove_file(&temporary) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(ApiError::internal(error.to_string())),
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn concurrent_first_writers_publish_one_complete_document_without_replacing_it() {
        let dir =
            std::env::temp_dir().join(format!("pisper-config-publish-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.json");
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
        let handles: Vec<_> = (0..2)
            .map(|id| {
                let path = path.clone();
                let barrier = barrier.clone();
                std::thread::spawn(move || {
                    barrier.wait();
                    create_object(&path, &serde_json::json!({"winner":id})).unwrap()
                })
            })
            .collect();
        assert_eq!(
            handles
                .into_iter()
                .filter(|handle| handle.thread().id() != std::thread::current().id())
                .map(|handle| usize::from(handle.join().unwrap()))
                .sum::<usize>(),
            1
        );
        let value: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert!(value["winner"] == 0 || value["winner"] == 1);
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
