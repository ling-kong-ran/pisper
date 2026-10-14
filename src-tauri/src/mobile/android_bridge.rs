use std::path::Path;

#[cfg(target_os = "android")]
pub(super) fn with_android_env<T>(
    action: impl FnOnce(&mut jni::JNIEnv, jni::objects::JObject) -> Result<T, String>,
) -> Result<T, String> {
    let context = tauri::tao::platform::android::prelude::main_android_context()
        .ok_or_else(|| "Android 上下文尚未就绪。".to_string())?;
    let vm = unsafe { jni::JavaVM::from_raw(context.java_vm.cast()) }
        .map_err(|error| format!("无法获取 Android JavaVM：{error}"))?;
    let mut env = vm
        .attach_current_thread_as_daemon()
        .map_err(|error| format!("无法附加 Android JNI 线程：{error}"))?;
    let object = unsafe { jni::objects::JObject::from_raw(context.context_jobject.cast()) };
    let result = action(&mut env, object);
    if result.is_err() {
        let _ = env.exception_clear();
    }
    result
}

#[cfg(target_os = "android")]
pub(super) fn android_set_trusted_proxy_port(port: u16) -> Result<(), String> {
    with_android_env(|env, context| {
        env.call_method(
            &context,
            "setTrustedProxyPort",
            "(I)V",
            &[jni::objects::JValue::Int(i32::from(port))],
        )
        .map_err(|error| format!("无法设置 Android 受信任代理端口：{error}"))?;
        Ok(())
    })
}

#[cfg(target_os = "android")]
pub(super) fn android_asset_exists(name: &str) -> bool {
    with_android_env(|env, context| {
        let assets = env
            .call_method(
                &context,
                "getAssets",
                "()Landroid/content/res/AssetManager;",
                &[],
            )
            .and_then(|value| value.l())
            .map_err(|error| format!("无法获取 Android Assets：{error}"))?;
        let name = env
            .new_string(name)
            .map(jni::objects::JObject::from)
            .map_err(|error| format!("无法构造 Asset 名称：{error}"))?;
        let stream = env
            .call_method(
                &assets,
                "open",
                "(Ljava/lang/String;)Ljava/io/InputStream;",
                &[jni::objects::JValue::Object(&name)],
            )
            .and_then(|value| value.l())
            .map_err(|error| format!("无法打开 Android Asset：{error}"))?;
        let _ = env.call_method(&stream, "close", "()V", &[]);
        Ok(())
    })
    .is_ok()
}

#[cfg(not(target_os = "android"))]
pub(super) fn android_asset_exists(_name: &str) -> bool {
    false
}

#[cfg(target_os = "android")]
pub(super) fn android_read_asset_prefix(name: &str, max_bytes: usize) -> Result<Vec<u8>, String> {
    use std::cmp::min;

    with_android_env(|env, context| {
        let assets = env
            .call_method(
                &context,
                "getAssets",
                "()Landroid/content/res/AssetManager;",
                &[],
            )
            .and_then(|value| value.l())
            .map_err(|error| format!("无法获取 Android Assets：{error}"))?;
        let name = env
            .new_string(name)
            .map(jni::objects::JObject::from)
            .map_err(|error| format!("无法构造 Asset 名称：{error}"))?;
        let stream = env
            .call_method(
                &assets,
                "open",
                "(Ljava/lang/String;)Ljava/io/InputStream;",
                &[jni::objects::JValue::Object(&name)],
            )
            .and_then(|value| value.l())
            .map_err(|error| format!("无法打开 Runtime Asset：{error}"))?;
        let result = (|| {
            let buffer_size = min(64 * 1024, max_bytes.max(1));
            let array = env
                .new_byte_array(buffer_size as i32)
                .map_err(|error| format!("无法分配 Asset 探测缓冲区：{error}"))?;
            let mut bytes = Vec::with_capacity(max_bytes);
            while bytes.len() < max_bytes {
                let requested = min(buffer_size, max_bytes - bytes.len());
                let read = env
                    .call_method(
                        &stream,
                        "read",
                        "([BII)I",
                        &[
                            jni::objects::JValue::Object(array.as_ref()),
                            jni::objects::JValue::Int(0),
                            jni::objects::JValue::Int(requested as i32),
                        ],
                    )
                    .and_then(|value| value.i())
                    .map_err(|error| format!("无法读取 Runtime Asset 首部：{error}"))?;
                if read == -1 {
                    break;
                }
                if read <= 0 || read as usize > requested {
                    return Err("Runtime Asset 首部读取未推进或长度无效。".into());
                }
                let chunk = env
                    .convert_byte_array(&array)
                    .map_err(|error| format!("无法转换 Runtime Asset 首部：{error}"))?;
                bytes.extend_from_slice(&chunk[..read as usize]);
            }
            Ok(bytes)
        })();
        close_android_asset(env, &stream, result)
    })
}

#[cfg(not(target_os = "android"))]
pub(super) fn android_read_asset_prefix(_name: &str, _max_bytes: usize) -> Result<Vec<u8>, String> {
    Err("当前平台没有 Android Runtime Asset。".into())
}

#[cfg(target_os = "android")]
pub(super) fn android_copy_asset(name: &str, target: &Path) -> Result<(), String> {
    use std::io::{BufWriter, Write};

    with_android_env(|env, context| {
        let assets = env
            .call_method(
                &context,
                "getAssets",
                "()Landroid/content/res/AssetManager;",
                &[],
            )
            .and_then(|value| value.l())
            .map_err(|error| format!("无法获取 Android Assets：{error}"))?;
        let name = env
            .new_string(name)
            .map(jni::objects::JObject::from)
            .map_err(|error| format!("无法构造 Asset 名称：{error}"))?;
        let stream = env
            .call_method(
                &assets,
                "open",
                "(Ljava/lang/String;)Ljava/io/InputStream;",
                &[jni::objects::JValue::Object(&name)],
            )
            .and_then(|value| value.l())
            .map_err(|error| format!("无法打开 Runtime Asset：{error}"))?;
        let result = (|| {
            let array = env
                .new_byte_array(64 * 1024)
                .map_err(|error| format!("无法分配 Asset 缓冲区：{error}"))?;
            let file = std::fs::File::create(target)
                .map_err(|error| format!("无法创建 Runtime 临时文件：{error}"))?;
            let mut writer = BufWriter::new(file);
            loop {
                let read = env
                    .call_method(
                        &stream,
                        "read",
                        "([B)I",
                        &[jni::objects::JValue::Object(array.as_ref())],
                    )
                    .and_then(|value| value.i())
                    .map_err(|error| format!("无法读取 Runtime Asset：{error}"))?;
                if read == -1 {
                    break;
                }
                if read <= 0 || read > 64 * 1024 {
                    return Err("Runtime Asset 读取未推进或长度无效。".into());
                }
                let bytes = env
                    .convert_byte_array(&array)
                    .map_err(|error| format!("无法转换 Runtime Asset：{error}"))?;
                writer
                    .write_all(&bytes[..read as usize])
                    .map_err(|error| format!("无法写入 Runtime Asset：{error}"))?;
            }
            writer
                .flush()
                .map_err(|error| format!("无法刷新 Runtime Asset：{error}"))?;
            Ok(())
        })();
        close_android_asset(env, &stream, result)
    })
}

#[cfg(target_os = "android")]
fn close_android_asset<T>(
    env: &mut jni::JNIEnv<'_>,
    stream: &jni::objects::JObject<'_>,
    result: Result<T, String>,
) -> Result<T, String> {
    // JNI 有待处理异常时不能调用 close；保留原始读取错误，再清异常并释放流。
    if result.is_err() {
        let _ = env.exception_clear();
    }
    let closed = env
        .call_method(stream, "close", "()V", &[])
        .map_err(|error| format!("无法关闭 Runtime Asset：{error}"));
    result.and_then(|value| closed.map(|_| value))
}

#[cfg(not(target_os = "android"))]
pub(super) fn android_copy_asset(_name: &str, _target: &Path) -> Result<(), String> {
    Err("当前平台没有 Android Runtime Asset。".into())
}
