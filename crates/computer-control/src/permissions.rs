// 这台机器现在能不能用 Computer Use。不能用时给出用户能照着做的原因。

#[derive(Clone, Debug, PartialEq)]
pub struct Support {
    pub supported: bool,
    /// 缺的系统权限（macOS）：`screen`、`accessibility`。
    pub missing_permissions: Vec<&'static str>,
    pub reason: Option<String>,
}

#[cfg(target_os = "macos")]
mod platform {
    use std::ffi::c_void;

    #[link(name = "CoreGraphics", kind = "framework")]
    extern "C" {
        fn CGPreflightScreenCaptureAccess() -> bool;
        fn CGRequestScreenCaptureAccess() -> bool;
    }
    #[link(name = "ApplicationServices", kind = "framework")]
    extern "C" {
        fn AXIsProcessTrusted() -> bool;
        fn AXIsProcessTrustedWithOptions(options: *const c_void) -> bool;
        static kAXTrustedCheckOptionPrompt: *const c_void;
    }
    #[link(name = "CoreFoundation", kind = "framework")]
    extern "C" {
        static kCFBooleanTrue: *const c_void;
        fn CFDictionaryCreate(
            allocator: *const c_void,
            keys: *const *const c_void,
            values: *const *const c_void,
            count: isize,
            key_callbacks: *const c_void,
            value_callbacks: *const c_void,
        ) -> *const c_void;
        fn CFRelease(value: *const c_void);
        static kCFTypeDictionaryKeyCallBacks: c_void;
        static kCFTypeDictionaryValueCallBacks: c_void;
    }

    pub fn missing() -> Vec<&'static str> {
        let mut missing = Vec::new();
        // SAFETY: 无参数的系统查询。
        unsafe {
            if !CGPreflightScreenCaptureAccess() {
                missing.push("screen");
            }
            if !AXIsProcessTrusted() {
                missing.push("accessibility");
            }
        }
        missing
    }

    pub fn request() {
        // SAFETY: 系统弹窗请求；字典只在这次调用里用，键值都是系统常量。
        unsafe {
            if !CGPreflightScreenCaptureAccess() {
                CGRequestScreenCaptureAccess();
            }
            if !AXIsProcessTrusted() {
                let keys = [kAXTrustedCheckOptionPrompt];
                let values = [kCFBooleanTrue];
                let options = CFDictionaryCreate(
                    std::ptr::null(),
                    keys.as_ptr(),
                    values.as_ptr(),
                    1,
                    std::ptr::addr_of!(kCFTypeDictionaryKeyCallBacks),
                    std::ptr::addr_of!(kCFTypeDictionaryValueCallBacks),
                );
                AXIsProcessTrustedWithOptions(options);
                CFRelease(options);
            }
        }
    }

    pub fn environment_problem() -> Option<String> {
        None
    }
}

#[cfg(target_os = "linux")]
mod platform {
    pub fn missing() -> Vec<&'static str> {
        Vec::new()
    }
    pub fn request() {}

    pub fn environment_problem() -> Option<String> {
        // XWayland 截不到也点不到原生 Wayland 窗口，假装能用只会让模型对着错的画面操作。
        if std::env::var("XDG_SESSION_TYPE").as_deref() == Ok("wayland")
            || std::env::var_os("WAYLAND_DISPLAY").is_some()
        {
            return Some("Computer use needs an X11 session; Wayland is not supported yet.".into());
        }
        if std::env::var_os("DISPLAY").is_none() {
            return Some("Computer use needs a graphical X11 session.".into());
        }
        x11rb::connect(None)
            .err()
            .map(|error| format!("The X11 display is not reachable: {error}"))
    }
}

#[cfg(target_os = "windows")]
mod platform {
    pub fn missing() -> Vec<&'static str> {
        Vec::new()
    }
    pub fn request() {}
    pub fn environment_problem() -> Option<String> {
        None
    }
}

#[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "linux")))]
mod platform {
    pub fn missing() -> Vec<&'static str> {
        Vec::new()
    }
    pub fn request() {}
    pub fn environment_problem() -> Option<String> {
        Some("Computer use is not supported on this platform.".into())
    }
}

pub fn check_support() -> Support {
    let reason = platform::environment_problem();
    let missing_permissions = platform::missing();
    Support {
        supported: reason.is_none() && missing_permissions.is_empty(),
        missing_permissions,
        reason,
    }
}

/// 弹出系统的授权请求（macOS）。其他平台什么也不做。
pub fn request_permissions() {
    platform::request();
}
