//! Real operating-system bindings for the implemented Node API surface. There is no
//! sandbox claim: a released plugin runs with the current OS user's access rights.
use super::{paths::display_path, PluginError, Result, MAX_PLUGIN_BYTES};
use base64::{engine::general_purpose, Engine};
use serde_json::{json, Value};
use std::{
    fs,
    io::{Read, Write},
    path::{Component, Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

pub(crate) const BOOTSTRAP: &str = include_str!("node-bootstrap.js");
pub(crate) fn dispatch(operation: &str, payload: &str) -> String {
    let result = serde_json::from_str::<Value>(payload)
        .map_err(PluginError::from)
        .and_then(|args| native(operation, &args));
    let value = match result {
        Ok(value) => json!({"ok":true,"value":value}),
        Err(error) => json!({"ok":false,"message":error.message}),
    };
    serde_json::to_string(&value).expect("native host result")
}
fn argument<'a>(args: &'a Value, index: usize) -> Result<&'a str> {
    args[index]
        .as_str()
        .ok_or_else(|| PluginError::new("ERR_INVALID_ARG_TYPE: expected a string"))
}
fn path(args: &Value, index: usize) -> Result<PathBuf> {
    Ok(PathBuf::from(argument(args, index)?))
}
fn bytes(value: &Value) -> Result<Vec<u8>> {
    value
        .as_array()
        .ok_or_else(|| PluginError::new("ERR_INVALID_ARG_TYPE: expected bytes"))?
        .iter()
        .map(|v| {
            v.as_u64()
                .filter(|v| *v <= 255)
                .map(|v| v as u8)
                .ok_or_else(|| PluginError::new("ERR_INVALID_ARG_TYPE: invalid byte"))
        })
        .collect()
}
fn normalize_path(value: &Path) -> PathBuf {
    let mut result = PathBuf::new();
    for component in value.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                if !result.pop() && !value.is_absolute() {
                    result.push("..");
                }
            }
            other => result.push(other.as_os_str()),
        }
    }
    if result.as_os_str().is_empty() {
        result.push(".");
    }
    result
}
fn io_error(error: std::io::Error, operation: &str, target: &Path) -> PluginError {
    let code = match error.kind() {
        std::io::ErrorKind::NotFound => "ENOENT",
        std::io::ErrorKind::AlreadyExists => "EEXIST",
        std::io::ErrorKind::PermissionDenied => "EACCES",
        std::io::ErrorKind::NotADirectory => "ENOTDIR",
        std::io::ErrorKind::IsADirectory => "EISDIR",
        _ => "EIO",
    };
    PluginError::new(format!("{code}: {operation}, '{}'", display_path(target)))
}
fn native(operation: &str, args: &Value) -> Result<Value> {
    match operation {
        "codec.encode" => {
            let source = argument(args, 0)?;
            let encoding = args[1].as_str().unwrap_or("utf8").to_ascii_lowercase();
            let output = match encoding.as_str() {
                "utf8" | "utf-8" => source.as_bytes().to_vec(),
                "utf16le" | "ucs2" | "ucs-2" => {
                    source.encode_utf16().flat_map(u16::to_le_bytes).collect()
                }
                "latin1" | "binary" | "ascii" => source.encode_utf16().map(|v| v as u8).collect(),
                "base64" => general_purpose::STANDARD
                    .decode(source)
                    .map_err(|e| PluginError::new(e.to_string()))?,
                "base64url" => general_purpose::URL_SAFE_NO_PAD
                    .decode(source)
                    .map_err(|e| PluginError::new(e.to_string()))?,
                "hex" => source
                    .as_bytes()
                    .chunks_exact(2)
                    .map(|v| {
                        std::str::from_utf8(v)
                            .ok()
                            .and_then(|v| u8::from_str_radix(v, 16).ok())
                            .ok_or_else(|| PluginError::new("Invalid hexadecimal input"))
                    })
                    .collect::<Result<Vec<_>>>()?,
                _ => {
                    return Err(PluginError::new(format!(
                        "ERR_UNKNOWN_ENCODING: {encoding}"
                    )))
                }
            };
            Ok(json!(output))
        }
        "codec.decode" => {
            let input = bytes(&args[0])?;
            let encoding = args[1].as_str().unwrap_or("utf8").to_ascii_lowercase();
            let output = match encoding.as_str() {
                "utf8" | "utf-8" => String::from_utf8_lossy(&input).into_owned(),
                "utf16le" | "ucs2" | "ucs-2" => String::from_utf16_lossy(
                    &input
                        .chunks_exact(2)
                        .map(|v| u16::from_le_bytes([v[0], v[1]]))
                        .collect::<Vec<_>>(),
                ),
                "latin1" | "binary" => input.iter().map(|v| char::from(*v)).collect(),
                "ascii" => input.iter().map(|v| char::from(*v & 127)).collect(),
                "base64" => general_purpose::STANDARD.encode(&input),
                "base64url" => general_purpose::URL_SAFE_NO_PAD.encode(&input),
                "hex" => input.iter().map(|v| format!("{v:02x}")).collect(),
                _ => {
                    return Err(PluginError::new(format!(
                        "ERR_UNKNOWN_ENCODING: {encoding}"
                    )))
                }
            };
            Ok(json!(output))
        }
        "fs.readFile" => {
            let path = path(args, 0)?;
            let file = fs::File::open(&path).map_err(|e| io_error(e, "open", &path))?;
            let mut output = Vec::new();
            file.take(128 * 1024 * 1024 + 1)
                .read_to_end(&mut output)
                .map_err(|e| io_error(e, "read", &path))?;
            if output.len() > 128 * 1024 * 1024 {
                return Err(PluginError::new("ERR_PISPER_NODE_COMPAT: native file buffer exceeds implemented 128 MiB boundary"));
            }
            Ok(json!(output))
        }
        "fs.writeFile" | "fs.appendFile" => {
            let path = path(args, 0)?;
            let input = bytes(&args[1])?;
            let flag = args[2].as_str().unwrap_or(if operation == "fs.appendFile" {
                "a"
            } else {
                "w"
            });
            let mut options = fs::OpenOptions::new();
            options.write(true);
            match flag {
                "w" | "w+" => {
                    options.create(true).truncate(true);
                }
                "wx" | "wx+" => {
                    options.create_new(true);
                }
                "a" | "a+" => {
                    options.create(true).append(true);
                }
                "ax" | "ax+" => {
                    options.create_new(true).append(true);
                }
                "r+" => {}
                _ => {
                    return Err(PluginError::new(format!(
                        "ERR_PISPER_NODE_COMPAT: unsupported fs flag {flag}"
                    )))
                }
            };
            let mut file = options
                .open(&path)
                .map_err(|e| io_error(e, "open", &path))?;
            file.write_all(&input)
                .map_err(|e| io_error(e, "write", &path))?;
            Ok(Value::Null)
        }
        "fs.mkdir" => {
            let path = path(args, 0)?;
            let recursive = args[1] == true;
            let result = if recursive {
                fs::create_dir_all(&path)
            } else {
                fs::create_dir(&path)
            };
            result.map_err(|e| io_error(e, "mkdir", &path))?;
            Ok(Value::Null)
        }
        "fs.readdir" => {
            let path = path(args, 0)?;
            let mut output = Vec::new();
            for entry in fs::read_dir(&path).map_err(|e| io_error(e, "scandir", &path))? {
                let entry = entry?;
                let kind = entry.file_type()?;
                output.push(json!({"name":entry.file_name().to_string_lossy(),"file":kind.is_file(),"directory":kind.is_dir(),"symlink":kind.is_symlink()}));
            }
            output.sort_by(|a, b| a["name"].as_str().cmp(&b["name"].as_str()));
            Ok(json!(output))
        }
        "fs.stat" | "fs.lstat" => {
            let path = path(args, 0)?;
            let metadata = if operation == "fs.lstat" {
                fs::symlink_metadata(&path)
            } else {
                fs::metadata(&path)
            }
            .map_err(|e| io_error(e, "stat", &path))?;
            let ms = |time: std::io::Result<SystemTime>| {
                time.ok()
                    .and_then(|v| v.duration_since(UNIX_EPOCH).ok())
                    .map(|v| v.as_secs_f64() * 1000.0)
                    .unwrap_or(0.0)
            };
            let output = json!({"size":metadata.len(),"file":metadata.is_file(),"directory":metadata.is_dir(),"symlink":metadata.file_type().is_symlink(),"mtimeMs":ms(metadata.modified()),"atimeMs":ms(metadata.accessed()),"birthtimeMs":ms(metadata.created()),"ctimeMs":ms(metadata.modified())});
            #[cfg(unix)]
            let mut output = output;
            #[cfg(unix)]
            {
                use std::os::unix::fs::MetadataExt;
                output["mode"] = json!(metadata.mode());
                output["uid"] = json!(metadata.uid());
                output["gid"] = json!(metadata.gid());
                output["ino"] = json!(metadata.ino());
                output["dev"] = json!(metadata.dev());
                output["nlink"] = json!(metadata.nlink());
            }
            Ok(output)
        }
        "fs.realpath" => {
            let path = path(args, 0)?;
            let canonical = fs::canonicalize(&path).map_err(|e| io_error(e, "realpath", &path))?;
            Ok(json!(display_path(&canonical)))
        }
        "fs.access" => {
            let path = path(args, 0)?;
            fs::metadata(&path).map_err(|e| io_error(e, "access", &path))?;
            if args[1].as_i64().unwrap_or(0) != 0 {
                return Err(PluginError::new(
                    "ERR_PISPER_NODE_COMPAT: access mode checks are not implemented",
                ));
            }
            Ok(Value::Null)
        }
        "fs.rename" => {
            let from = path(args, 0)?;
            let to = path(args, 1)?;
            fs::rename(&from, &to).map_err(|e| io_error(e, "rename", &from))?;
            Ok(Value::Null)
        }
        "fs.copyFile" => {
            let from = path(args, 0)?;
            let to = path(args, 1)?;
            if args[2].as_u64().unwrap_or(0) & 1 != 0 {
                let mut target = fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(&to)
                    .map_err(|e| io_error(e, "copyfile", &to))?;
                std::io::copy(&mut fs::File::open(&from)?, &mut target)?;
            } else {
                fs::copy(&from, &to).map_err(|e| io_error(e, "copyfile", &from))?;
            }
            Ok(Value::Null)
        }
        "fs.unlink" | "fs.rmdir" | "fs.rm" => {
            let path = path(args, 0)?;
            let recursive = args[1] == true;
            let force = args[2] == true;
            let result = match fs::symlink_metadata(&path) {
                Ok(meta) if meta.is_dir() && !meta.file_type().is_symlink() => {
                    if recursive {
                        fs::remove_dir_all(&path)
                    } else {
                        fs::remove_dir(&path)
                    }
                }
                Ok(_) => fs::remove_file(&path),
                Err(e) => Err(e),
            };
            if let Err(error) = result {
                if !(force && error.kind() == std::io::ErrorKind::NotFound) {
                    return Err(io_error(error, "rm", &path));
                }
            }
            Ok(Value::Null)
        }
        "path.join" => {
            let mut result = PathBuf::new();
            for value in args.as_array().into_iter().flatten() {
                let value = value
                    .as_str()
                    .ok_or_else(|| PluginError::new("ERR_INVALID_ARG_TYPE: path must be string"))?;
                if !value.is_empty() {
                    if result.as_os_str().is_empty() {
                        result.push(value);
                    } else {
                        let sep = std::path::MAIN_SEPARATOR;
                        result = PathBuf::from(format!("{}{sep}{value}", result.display()));
                    }
                }
            }
            Ok(json!(normalize_path(&result).to_string_lossy()))
        }
        "path.resolve" => {
            let mut result = std::env::current_dir()?;
            for value in args.as_array().into_iter().flatten() {
                let value = Path::new(value.as_str().ok_or_else(|| {
                    PluginError::new("ERR_INVALID_ARG_TYPE: path must be string")
                })?);
                if value.is_absolute() {
                    result = value.to_owned();
                } else {
                    result.push(value);
                }
            }
            Ok(json!(normalize_path(&result).to_string_lossy()))
        }
        "path.normalize" => Ok(json!(normalize_path(&path(args, 0)?).to_string_lossy())),
        "path.dirname" => Ok(json!(path(args, 0)?
            .parent()
            .filter(|v| !v.as_os_str().is_empty())
            .map(|v| v.to_string_lossy().into_owned())
            .unwrap_or_else(|| ".".into()))),
        "path.basename" => {
            let path = path(args, 0)?;
            let mut name = path
                .file_name()
                .map(|v| v.to_string_lossy().into_owned())
                .unwrap_or_default();
            if let Some(suffix) = args[1].as_str() {
                if let Some(short) = name.strip_suffix(suffix) {
                    name = short.into();
                }
            }
            Ok(json!(name))
        }
        "path.extname" => {
            let path = path(args, 0)?;
            Ok(json!(path
                .extension()
                .map(|v| format!(".{}", v.to_string_lossy()))
                .unwrap_or_default()))
        }
        "path.isAbsolute" => Ok(json!(path(args, 0)?.is_absolute())),
        "process.cwd" => Ok(json!(std::env::current_dir()?.to_string_lossy())),
        "process.platform" => Ok(json!(if cfg!(windows) {
            "win32"
        } else if cfg!(target_os = "macos") {
            "darwin"
        } else {
            std::env::consts::OS
        })),
        "process.arch" => Ok(json!(match std::env::consts::ARCH {
            "x86_64" => "x64",
            "aarch64" => "arm64",
            "x86" => "ia32",
            other => other,
        })),
        "process.pid" => Ok(json!(std::process::id())),
        "os.tmpdir" => Ok(json!(std::env::temp_dir().to_string_lossy())),
        "module.read" => {
            let target = path(args, 0)?;
            let metadata = fs::metadata(&target)?;
            if !metadata.is_file() || metadata.len() > MAX_PLUGIN_BYTES as u64 {
                return Err(PluginError::new(
                    "ERR_PISPER_NODE_COMPAT: JavaScript module file is invalid or too large",
                ));
            }
            Ok(json!(String::from_utf8_lossy(&fs::read(target)?)))
        }
        "module.resolve" => Ok(json!(resolve_module(
            Path::new(argument(args, 0)?),
            argument(args, 1)?
        )?)),
        _ => Err(PluginError::new(format!(
            "ERR_PISPER_NODE_COMPAT: native API {operation} is not implemented"
        ))),
    }
}
pub(crate) fn resolve_module(base: &Path, name: &str) -> Result<String> {
    let bare = name.strip_prefix("node:").unwrap_or(name);
    if [
        "fs",
        "fs/promises",
        "path",
        "buffer",
        "process",
        "timers",
        "timers/promises",
        "os",
    ]
    .contains(&bare)
    {
        return Ok(format!("node:{bare}"));
    }
    if name.starts_with("node:") {
        return Err(PluginError::new(format!(
            "ERR_PISPER_NODE_COMPAT: Node built-in {name} is not implemented"
        )));
    }
    let requested = Path::new(name);
    let path = if requested.is_absolute() {
        requested.to_owned()
    } else if name.starts_with('.') {
        base.parent().unwrap_or(Path::new(".")).join(requested)
    } else {
        return Err(PluginError::new(format!(
            "ERR_PISPER_NODE_COMPAT: npm package resolution for '{name}' is not implemented"
        )));
    };
    let resolved = fs::canonicalize(&path).map_err(|e| io_error(e, "import", &path))?;
    if !fs::metadata(&resolved)?.is_file() {
        return Err(PluginError::new(
            "ERR_MODULE_NOT_FOUND: module is not a file",
        ));
    }
    Ok(resolved.to_string_lossy().into_owned())
}
pub(crate) fn is_commonjs(path: &Path, source: &str) -> bool {
    match path.extension().and_then(|v| v.to_str()).unwrap_or("") {
        "cjs" => true,
        "mjs" => false,
        "js" => {
            let mut parent = path.parent();
            while let Some(directory) = parent {
                if let Ok(bytes) = fs::read(directory.join("package.json")) {
                    if let Ok(value) = serde_json::from_slice::<Value>(&bytes) {
                        if let Some(kind) = value["type"].as_str() {
                            return kind != "module";
                        }
                    }
                }
                parent = directory.parent();
            }
            !regex::Regex::new(r"(?m)^\s*(?:export\s|import\s|import\{)")
                .expect("module syntax probe")
                .is_match(source)
        }
        _ => false,
    }
}
