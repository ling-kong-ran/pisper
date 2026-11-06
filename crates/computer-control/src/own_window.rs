// Agent 不能操作 Pisper 自己：它能点到审批卡片上的“批准”，等于自己给自己批准。
// 这一层只看系统告诉我们鼠标下面是谁、前台是谁，不依赖模型遵守提示词。
// macOS 上要在主线程调用。

#[cfg(target_os = "windows")]
mod platform {
    use windows_sys::Win32::Foundation::POINT;
    use windows_sys::Win32::System::Threading::GetCurrentProcessId;
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        GetAncestor, GetForegroundWindow, GetWindowLongPtrW, GetWindowThreadProcessId,
        WindowFromPoint, GA_ROOT, GWL_EXSTYLE, WS_EX_TRANSPARENT,
    };

    fn belongs_to_us(window: windows_sys::Win32::Foundation::HWND) -> bool {
        if window.is_null() {
            return false;
        }
        let mut process = 0;
        // SAFETY: window 是系统刚返回的句柄；失效时调用只是返回 0。
        unsafe {
            GetWindowThreadProcessId(window, &mut process);
            process == GetCurrentProcessId()
        }
    }

    pub fn point_hits_own_window(x: i32, y: i32) -> bool {
        // SAFETY: 都是按值传参的查询。遮罩是鼠标穿透的（WS_EX_TRANSPARENT），点击不会落在它上面，
        // 系统一般也不会报它；万一报了，不能把每次点击都当成点到了 Pisper。
        unsafe {
            let window = GetAncestor(WindowFromPoint(POINT { x, y }), GA_ROOT);
            if window.is_null()
                || GetWindowLongPtrW(window, GWL_EXSTYLE) as u32 & WS_EX_TRANSPARENT != 0
            {
                return false;
            }
            belongs_to_us(window)
        }
    }

    pub fn foreground_is_own() -> bool {
        // SAFETY: 无参数的查询。
        belongs_to_us(unsafe { GetForegroundWindow() })
    }
}

#[cfg(target_os = "macos")]
mod platform {
    use objc2::msg_send;
    use objc2::runtime::{AnyClass, AnyObject, Bool};

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct NSPoint {
        x: f64,
        y: f64,
    }
    // SAFETY: 和系统头文件里的 CGPoint 布局一致，编码也按它写。
    unsafe impl objc2::Encode for NSPoint {
        const ENCODING: objc2::Encoding =
            objc2::Encoding::Struct("CGPoint", &[f64::ENCODING, f64::ENCODING]);
    }

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct CGPointRaw {
        x: f64,
        y: f64,
    }
    #[repr(C)]
    #[derive(Clone, Copy)]
    struct CGSizeRaw {
        width: f64,
        height: f64,
    }
    #[repr(C)]
    #[derive(Clone, Copy)]
    struct CGRectRaw {
        origin: CGPointRaw,
        size: CGSizeRaw,
    }
    #[link(name = "CoreGraphics", kind = "framework")]
    extern "C" {
        fn CGMainDisplayID() -> u32;
        fn CGDisplayBounds(display: u32) -> CGRectRaw;
    }

    pub fn point_hits_own_window(x: i32, y: i32) -> bool {
        let (Some(window_class), Some(app_class)) =
            (AnyClass::get(c"NSWindow"), AnyClass::get(c"NSApplication"))
        else {
            return false;
        };
        // SAFETY: 这些都是 AppKit 的公开类方法，调用方保证在主线程。
        // AppKit 的屏幕坐标原点在左下角，输入坐标在左上角，要翻一下。
        unsafe {
            let height = CGDisplayBounds(CGMainDisplayID()).size.height;
            let point = NSPoint {
                x: f64::from(x),
                y: height - f64::from(y),
            };
            let hit: isize = msg_send![window_class, windowNumberAtPoint: point, belowWindowWithWindowNumber: 0isize];
            let app: *mut AnyObject = msg_send![app_class, sharedApplication];
            let windows: *mut AnyObject = msg_send![app, windows];
            let count: usize = msg_send![windows, count];
            (0..count).any(|index| {
                let window: *mut AnyObject = msg_send![windows, objectAtIndex: index];
                let number: isize = msg_send![window, windowNumber];
                number == hit
            })
        }
    }

    pub fn foreground_is_own() -> bool {
        let Some(app_class) = AnyClass::get(c"NSApplication") else {
            return false;
        };
        // SAFETY: AppKit 的公开方法，调用方保证在主线程。
        unsafe {
            let app: *mut AnyObject = msg_send![app_class, sharedApplication];
            let active: Bool = msg_send![app, isActive];
            active.as_bool()
        }
    }
}

#[cfg(target_os = "linux")]
mod platform {
    use x11rb::connection::Connection;
    use x11rb::protocol::xproto::{AtomEnum, ConnectionExt, Window};

    fn atom(connection: &impl Connection, name: &[u8]) -> Option<u32> {
        Some(connection.intern_atom(false, name).ok()?.reply().ok()?.atom)
    }

    fn pid_of(connection: &impl Connection, window: Window, wm_pid: u32) -> Option<u32> {
        connection
            .get_property(false, window, wm_pid, AtomEnum::CARDINAL, 0, 1)
            .ok()?
            .reply()
            .ok()?
            .value32()?
            .next()
    }

    // 有窗口管理器时顶层是它的外框，外框上没有 PID，要往下找真正的应用窗口。
    fn subtree_belongs_to(
        connection: &impl Connection,
        window: Window,
        wm_pid: u32,
        pid: u32,
        depth: u8,
    ) -> bool {
        if pid_of(connection, window, wm_pid) == Some(pid) {
            return true;
        }
        if depth == 0 {
            return false;
        }
        connection
            .query_tree(window)
            .ok()
            .and_then(|cookie| cookie.reply().ok())
            .is_some_and(|tree| {
                tree.children
                    .iter()
                    .any(|child| subtree_belongs_to(connection, *child, wm_pid, pid, depth - 1))
            })
    }

    pub fn point_hits_own_window(x: i32, y: i32) -> bool {
        let Ok((connection, screen)) = x11rb::connect(None) else {
            return false;
        };
        let root = connection.setup().roots[screen].root;
        let (Some(wm_pid), Ok(cookie)) = (
            atom(&connection, b"_NET_WM_PID"),
            connection.translate_coordinates(root, root, x as i16, y as i16),
        ) else {
            return false;
        };
        let Ok(reply) = cookie.reply() else {
            return false;
        };
        reply.child != 0
            && subtree_belongs_to(&connection, reply.child, wm_pid, std::process::id(), 3)
    }

    pub fn foreground_is_own() -> bool {
        let Ok((connection, screen)) = x11rb::connect(None) else {
            return false;
        };
        let root = connection.setup().roots[screen].root;
        let (Some(wm_pid), Some(active_atom)) = (
            atom(&connection, b"_NET_WM_PID"),
            atom(&connection, b"_NET_ACTIVE_WINDOW"),
        ) else {
            return false;
        };
        let active = connection
            .get_property(false, root, active_atom, AtomEnum::WINDOW, 0, 1)
            .ok()
            .and_then(|cookie| cookie.reply().ok())
            .and_then(|reply| reply.value32().and_then(|mut values| values.next()));
        active.is_some_and(|window| {
            window != 0 && pid_of(&connection, window, wm_pid) == Some(std::process::id())
        })
    }
}

#[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "linux")))]
mod platform {
    pub fn point_hits_own_window(_x: i32, _y: i32) -> bool {
        false
    }
    pub fn foreground_is_own() -> bool {
        false
    }
}

pub use platform::{foreground_is_own, point_hits_own_window};
