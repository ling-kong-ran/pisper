use super::{error::engine, native::NativeInference};
use serde_json::{json, Value};
use std::io::{BufRead, Read, Write};
/// Called before ordinary runtime/config initialization. The worker only sees parent-supplied, verified model paths.
pub fn worker_main() -> i32 {
    let input = std::io::stdin();
    let mut input = input.lock();
    let output = std::io::stdout();
    let mut output = output.lock();
    let mut inference = None::<NativeInference>;
    loop {
        let mut line = String::new();
        let read = Read::by_ref(&mut input)
            .take(44_000_000)
            .read_line(&mut line);
        if matches!(read, Ok(0)) {
            break;
        }
        if read.is_err() || !line.ends_with('\n') {
            return 2;
        }
        let Ok(message) = serde_json::from_str::<Value>(&line) else {
            return 2;
        };
        let method = message["method"].as_str().unwrap_or("");
        if method == "shutdown" {
            break;
        }
        let result = match method {
            "init" if inference.is_none() => {
                NativeInference::new(&message["params"]).map(|engine| {
                    inference = Some(engine);
                    json!({"ready":true})
                })
            }
            "version" => message["params"]["nativeLibraryDir"]
                .as_str()
                .map(std::path::Path::new)
                .ok_or_else(|| engine("config"))
                .and_then(NativeInference::version),
            _ => inference
                .as_mut()
                .ok_or_else(|| engine("config"))
                .and_then(|engine| engine.handle(method, &message["params"])),
        };
        let response = match result {
            Ok(result) => json!({"id":message["id"],"ok":true,"result":result}),
            Err(error) => json!({"id":message["id"],"ok":false,"error":{"code":error.code}}),
        };
        let Ok(bytes) = serde_json::to_vec(&response) else {
            return 2;
        };
        if output.write_all(&bytes).is_err()
            || output.write_all(b"\n").is_err()
            || output.flush().is_err()
        {
            return 2;
        }
    }
    drop(inference);
    0
}
