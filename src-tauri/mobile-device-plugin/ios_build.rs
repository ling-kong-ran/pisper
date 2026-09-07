use std::{env, error::Error, fs, io::Read, path::Path, path::PathBuf, process::Command};

type BuildResult<T> = Result<T, Box<dyn Error>>;

#[derive(Debug, PartialEq)]
struct IosTarget {
    sdk: &'static str,
    triple: String,
    clang_runtime: &'static str,
}

fn ios_target(target: &str, minimum: &str) -> BuildResult<IosTarget> {
    let (arch, simulator) = match target {
        "aarch64-apple-ios" => ("arm64", false),
        "aarch64-apple-ios-sim" => ("arm64", true),
        "x86_64-apple-ios" => ("x86_64", true),
        _ => return Err(format!("unsupported iOS Rust target: {target}").into()),
    };
    Ok(IosTarget {
        sdk: if simulator {
            "iphonesimulator"
        } else {
            "iphoneos"
        },
        triple: format!(
            "{arch}-apple-ios{minimum}{}",
            if simulator { "-simulator" } else { "" }
        ),
        clang_runtime: if simulator {
            "clang_rt.iossim"
        } else {
            "clang_rt.ios"
        },
    })
}

fn output(command: &mut Command) -> BuildResult<String> {
    let result = command.output()?;
    if !result.status.success() {
        return Err(format!(
            "{command:?} failed: {}",
            String::from_utf8_lossy(&result.stderr)
        )
        .into());
    }
    Ok(String::from_utf8(result.stdout)?.trim().to_owned())
}

fn copy_api(source: &Path, destination: &Path) -> BuildResult<()> {
    fs::create_dir_all(destination)?;
    for entry in fs::read_dir(source)? {
        let entry = entry?;
        if [".build", "Package.resolved", "Tests"]
            .iter()
            .any(|name| entry.file_name() == *name)
        {
            continue;
        }
        if entry.file_type()?.is_dir() {
            copy_api(&entry.path(), &destination.join(entry.file_name()))?;
        } else {
            fs::copy(entry.path(), destination.join(entry.file_name()))?;
            println!("cargo:rerun-if-changed={}", entry.path().display());
        }
    }
    Ok(())
}

fn is_fat_macho(header: &[u8]) -> bool {
    matches!(
        header.get(..4),
        Some([0xca, 0xfe, 0xba, 0xbe])
            | Some([0xbe, 0xba, 0xfe, 0xca])
            | Some([0xca, 0xfe, 0xba, 0xbf])
            | Some([0xbf, 0xba, 0xfe, 0xca])
    )
}

fn link_binary_artifacts(scratch: &Path, bin: &Path, target: &IosTarget) -> BuildResult<()> {
    let arch = target
        .triple
        .split('-')
        .next()
        .ok_or("missing target architecture")?;
    let artifacts = [
        (
            "sherpa-onnx/SherpaOnnxIOS/sherpa-onnx.xcframework",
            "pisper_sherpa",
        ),
        (
            "onnxruntime-libs/OnnxruntimeIOS/onnxruntime.xcframework",
            "pisper_onnx",
        ),
        (
            "libarchive.xcframework/libarchive/libarchive.xcframework",
            "pisper_archive",
        ),
    ];
    for (relative, name) in artifacts {
        let framework = scratch.join("artifacts").join(relative);
        let json = output(
            Command::new("plutil")
                .args(["-convert", "json", "-o", "-"])
                .arg(framework.join("Info.plist")),
        )?;
        let info: serde_json::Value = serde_json::from_str(&json)?;
        let slice = info["AvailableLibraries"]
            .as_array()
            .ok_or("missing XCFramework slices")?
            .iter()
            .find(|slice| {
                slice["SupportedPlatform"] == "ios"
                    && slice["SupportedPlatformVariant"].as_str()
                        == if target.sdk == "iphonesimulator" {
                            Some("simulator")
                        } else {
                            None
                        }
                    && slice["SupportedArchitectures"]
                        .as_array()
                        .is_some_and(|values| values.iter().any(|value| value == arch))
            })
            .ok_or("missing matching iOS XCFramework slice")?;
        let source = framework
            .join(
                slice["LibraryIdentifier"]
                    .as_str()
                    .ok_or("missing slice identifier")?,
            )
            .join(slice["BinaryPath"].as_str().ok_or("missing slice binary")?);
        let library = bin.join(format!("lib{name}.a"));
        // Rust staticlib 不会自动收集 SwiftPM 二进制依赖，显式打包经过 SPM 校验的静态切片。
        let architectures = output(Command::new("xcrun").args(["lipo", "-archs"]).arg(&source))?;
        if !architectures.split_whitespace().any(|value| value == arch) {
            return Err(
                format!("missing {arch} in XCFramework binary: {}", source.display()).into(),
            );
        }
        // ONNX 真机切片可能只有一个架构但仍是 fat 容器，不能按架构数量决定是否拆包。
        let mut header = [0; 8];
        fs::File::open(&source)?.read_exact(&mut header)?;
        if is_fat_macho(&header) {
            output(
                Command::new("xcrun")
                    .args(["lipo", "-thin", arch])
                    .arg(&source)
                    .arg("-output")
                    .arg(&library),
            )?;
        } else {
            fs::copy(&source, &library)?;
        }
        if !fs::read(&library)?.starts_with(b"!<arch>\n") {
            return Err(
                format!("expected static XCFramework archive: {}", source.display()).into(),
            );
        }
        println!("cargo:rustc-link-lib=static={name}");
    }
    for library in ["c++", "z", "bz2", "iconv", "xml2"] {
        println!("cargo:rustc-link-lib={library}");
    }
    for framework in [
        "AVFoundation",
        "CoreFoundation",
        "Foundation",
        "CoreML",
        "Accelerate",
    ] {
        println!("cargo:rustc-link-lib=framework={framework}");
    }
    Ok(())
}

fn stage_resource_bundle(bin: &Path, name: &str) -> BuildResult<()> {
    let bundle_name = format!("{name}_{name}.bundle");
    let source = bin.join(&bundle_name);
    if !source.is_dir() {
        return Err("missing Swift resource bundle".into());
    }
    if let (Some(products), Some(resources)) = (
        env::var_os("TARGET_BUILD_DIR"),
        env::var_os("UNLOCALIZED_RESOURCES_FOLDER_PATH"),
    ) {
        let destination = PathBuf::from(products).join(resources).join(bundle_name);
        if destination.exists() {
            fs::remove_dir_all(&destination)?;
        }
        copy_api(&source, &destination)?;
        // Xcode clean 删除 App 后，即使 Rust 源未变也必须重新放入 Bundle.module 资源。
        println!("cargo:rerun-if-changed={}", destination.display());
    } else {
        println!(
            "cargo:warning=Swift resource bundle requires Xcode staging: {}",
            source.display()
        );
    }
    Ok(())
}

pub fn build() -> BuildResult<()> {
    for key in [
        "DEP_TAURI_IOS_LIBRARY_PATH",
        "IPHONEOS_DEPLOYMENT_TARGET",
        "PISPER_IOS_SWIFT_CACHE_PATH",
        "TARGET_BUILD_DIR",
        "UNLOCALIZED_RESOURCES_FOLDER_PATH",
    ] {
        println!("cargo:rerun-if-env-changed={key}");
    }
    let minimum = env::var("IPHONEOS_DEPLOYMENT_TARGET").unwrap_or_else(|_| "15.1".into());
    let target = ios_target(&env::var("TARGET")?, &minimum)?;
    let manifest =
        PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").ok_or("missing manifest directory")?);
    let package = manifest.join("ios");
    let name = env::var("CARGO_PKG_NAME")?;
    let api =
        PathBuf::from(env::var_os("DEP_TAURI_IOS_LIBRARY_PATH").ok_or("missing Tauri iOS API")?);
    let staged_api = manifest.join(".tauri/tauri-api");
    if staged_api.exists() {
        fs::remove_dir_all(&staged_api)?;
    }
    copy_api(&api, &staged_api)?;
    println!(
        "cargo:rerun-if-changed={}",
        package.join("Package.swift").display()
    );
    println!(
        "cargo:rerun-if-changed={}",
        package.join("Sources").display()
    );

    let sdk = output(Command::new("xcrun").args(["--sdk", target.sdk, "--show-sdk-path"]))?;
    let scratch = PathBuf::from(env::var_os("OUT_DIR").ok_or("missing OUT_DIR")?)
        .join("swift-rs")
        .join(&name);
    let configuration = if env::var("DEBUG").as_deref() == Ok("true") {
        "debug"
    } else {
        "release"
    };
    let mut swift = Command::new("xcrun");
    // --triple 必须作用于 SPM 本身，才能选择 iOS XCFramework 切片和正确的依赖图。
    swift
        .env_remove("SDKROOT")
        .current_dir(&package)
        .args([
            "swift",
            "build",
            "--triple",
            &target.triple,
            "--sdk",
            &sdk,
            "--configuration",
            configuration,
            "--product",
            &name,
            "--disable-keychain",
            "--disable-netrc",
            "--scratch-path",
        ])
        .arg(&scratch);
    if let Some(cache) = env::var_os("PISPER_IOS_SWIFT_CACHE_PATH") {
        swift.arg("--cache-path").arg(cache);
    }
    println!("cargo:warning=Building Swift plugin for {}", target.triple);
    if !swift.status()?.success() {
        return Err("Swift iOS package compilation failed".into());
    }
    // 由 SPM 返回实际输出目录，不再假定宿主架构或 arm64-apple-macosx 路径。
    let bin = PathBuf::from(output(swift.arg("--show-bin-path"))?);
    if !bin.join(format!("lib{name}.a")).is_file() {
        return Err(format!("missing Swift static library in {}", bin.display()).into());
    }
    println!("cargo:rustc-link-search=native={}", bin.display());
    println!("cargo:rustc-link-lib=static={name}");
    link_binary_artifacts(&scratch, &bin, &target)?;
    stage_resource_bundle(&bin, &name)?;

    let compiler = PathBuf::from(output(Command::new("xcrun").args(["--find", "swiftc"]))?);
    let toolchain = compiler
        .parent()
        .and_then(Path::parent)
        .ok_or("invalid Swift compiler path")?;
    println!(
        "cargo:rustc-link-search=native={}",
        toolchain.join("lib/swift").join(target.sdk).display()
    );
    println!("cargo:rustc-link-search=native=/usr/lib/swift");
    let clang_resources = output(Command::new("xcrun").args(["clang", "--print-resource-dir"]))?;
    println!(
        "cargo:rustc-link-search=native={}",
        Path::new(&clang_resources).join("lib/darwin").display()
    );
    println!("cargo:rustc-link-lib={}", target.clang_runtime);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fat_container_detection_includes_single_architecture_archives() {
        assert!(is_fat_macho(&[0xca, 0xfe, 0xba, 0xbe, 0, 0, 0, 1]));
        assert!(is_fat_macho(&[0xca, 0xfe, 0xba, 0xbe, 0, 0, 0, 2]));
        assert!(is_fat_macho(&[0xbe, 0xba, 0xfe, 0xca]));
        assert!(is_fat_macho(&[0xca, 0xfe, 0xba, 0xbf]));
        assert!(is_fat_macho(&[0xbf, 0xba, 0xfe, 0xca]));
        assert!(!is_fat_macho(b"!<arch>\n"));
        assert!(!is_fat_macho(&[0xcf, 0xfa, 0xed, 0xfe]));
        assert!(!is_fat_macho(&[]));
    }

    #[test]
    fn target_mapping_uses_target_architecture_and_platform() {
        assert_eq!(
            ios_target("aarch64-apple-ios", "15.1").unwrap(),
            IosTarget {
                sdk: "iphoneos",
                triple: "arm64-apple-ios15.1".into(),
                clang_runtime: "clang_rt.ios",
            }
        );
        assert_eq!(
            ios_target("aarch64-apple-ios-sim", "15.1").unwrap(),
            IosTarget {
                sdk: "iphonesimulator",
                triple: "arm64-apple-ios15.1-simulator".into(),
                clang_runtime: "clang_rt.iossim",
            }
        );
        assert_eq!(
            ios_target("x86_64-apple-ios", "16.0").unwrap().triple,
            "x86_64-apple-ios16.0-simulator"
        );
        assert!(ios_target("aarch64-apple-darwin", "15.1").is_err());
        assert!(ios_target("aarch64-linux-android", "15.1").is_err());
    }
}
