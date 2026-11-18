// WebView2 的完成回调不能销毁 WebView。Tauri 的 run_on_main_thread 在主线程会立即执行，
// 因此这里用独立的消息窗口强制排队；对象始终留在原线程，不跨线程搬运 COM 对象。
use std::{any::Any, cell::RefCell};
use windows::{
    core::w,
    Win32::{
        Foundation::{HWND, LPARAM, LRESULT, WPARAM},
        UI::{
            Shell::{DefSubclassProc, SetWindowSubclass},
            WindowsAndMessaging::{
                CreateWindowExW, DestroyWindow, PostMessageW, HWND_MESSAGE, WINDOW_EX_STYLE,
                WINDOW_STYLE, WM_APP,
            },
        },
    },
};

const DISPOSE: u32 = WM_APP + 1;
const SUBCLASS_ID: usize = 1;

struct Dispatcher {
    hwnd: HWND,
    pending: Vec<Box<dyn Any>>,
}

impl Drop for Dispatcher {
    fn drop(&mut self) {
        // SAFETY: 线程局部窗口只能在创建它的线程上销毁。退出时系统回收尚未处理的消息。
        unsafe {
            let _ = DestroyWindow(self.hwnd);
        }
    }
}

thread_local! {
    static DISPATCHER: RefCell<Option<Dispatcher>> = const { RefCell::new(None) };
}

// 回调完成、同步失败和取消释放回调，都通过同一条延迟释放路径。
pub(super) struct DeferredDrop<T: Any>(Option<T>);

impl<T: Any> DeferredDrop<T> {
    pub(super) fn new(value: T) -> Self {
        Self(Some(value))
    }

    pub(super) fn get(&self) -> Option<&T> {
        self.0.as_ref()
    }
}

impl<T: Any> Drop for DeferredDrop<T> {
    fn drop(&mut self) {
        if let Some(value) = self.0.take() {
            defer_drop(value);
        }
    }
}

// 在建第一个 WebView 之前准备好：不能等到回调里才发现无法安排安全销毁。
pub(super) fn initialize() -> Result<(), String> {
    DISPATCHER.with_borrow_mut(|slot| {
        if slot.is_some() {
            return Ok(());
        }
        // SAFETY: STATIC 是系统窗口类；消息窗口与调用者同线程，不显示，也不接受网页输入。
        unsafe {
            let hwnd = CreateWindowExW(
                WINDOW_EX_STYLE::default(),
                w!("STATIC"),
                w!(""),
                WINDOW_STYLE::default(),
                0,
                0,
                0,
                0,
                Some(HWND_MESSAGE),
                None,
                None,
                None,
            )
            .map_err(|error| error.to_string())?;
            if !SetWindowSubclass(hwnd, Some(dispatch), SUBCLASS_ID, 0).as_bool() {
                let _ = DestroyWindow(hwnd);
                return Err("Could not initialize browser disposal queue.".into());
            }
            *slot = Some(Dispatcher {
                hwnd,
                pending: Vec::new(),
            });
        }
        Ok(())
    })
}

fn defer_drop(value: impl Any) {
    let mut pending = Some(Box::new(value) as Box<dyn Any>);
    let _ = DISPATCHER.try_with(|slot| {
        let Ok(mut slot) = slot.try_borrow_mut() else {
            return;
        };
        let Some(dispatcher) = slot.as_mut() else {
            return;
        };
        if let Some(value) = pending.take() {
            dispatcher.pending.push(value);
        }
        // SAFETY: 仅投递通知，不传裸指针。投递失败保留所有权，下次投递或线程退出时再回收。
        if unsafe { PostMessageW(Some(dispatcher.hwnd), DISPOSE, WPARAM(0), LPARAM(0)) }.is_err() {
            eprintln!("Could not post browser disposal notification.");
        }
    });
    // 线程局部队列正在析构时，取消的 COM 回调仍可能释放 guard。交给进程回收，避免重入析构或 panic。
    if let Some(value) = pending {
        std::mem::forget(value);
    }
}

unsafe extern "system" fn dispatch(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    _id: usize,
    _data: usize,
) -> LRESULT {
    if msg == DISPOSE {
        let pending = DISPATCHER.with_borrow_mut(|slot| {
            slot.as_mut()
                .map(|dispatcher| std::mem::take(&mut dispatcher.pending))
        });
        // 销毁可能重入消息循环，必须先结束 RefCell 借用。
        drop(pending);
        return LRESULT(0);
    }
    // SAFETY: 原样转交当前窗口的子类过程参数。
    unsafe { DefSubclassProc(hwnd, msg, wparam, lparam) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{cell::Cell, rc::Rc, thread};
    use windows::Win32::UI::WindowsAndMessaging::{DispatchMessageW, PeekMessageW, MSG, PM_REMOVE};

    struct Probe(Rc<Cell<usize>>, thread::ThreadId);
    impl Drop for Probe {
        fn drop(&mut self) {
            assert_eq!(thread::current().id(), self.1);
            self.0.set(self.0.get() + 1);
        }
    }

    fn dispatch_one() {
        let hwnd = DISPATCHER.with_borrow(|slot| slot.as_ref().unwrap().hwnd);
        // SAFETY: 只取这个测试线程的队列窗口消息，不等待，也不使用真实 WebView 或网络。
        unsafe {
            let mut msg = MSG::default();
            assert!(PeekMessageW(&mut msg, Some(hwnd), DISPOSE, DISPOSE, PM_REMOVE).as_bool());
            DispatchMessageW(&msg);
        }
    }

    #[test]
    fn disposal_is_queued_even_on_the_owner_thread() {
        initialize().unwrap();
        let count = Rc::new(Cell::new(0));
        let browser = Rc::new(Probe(count.clone(), thread::current().id()));
        let weak = Rc::downgrade(&browser);
        drop(DeferredDrop::new(browser));
        assert!(
            weak.upgrade().is_some(),
            "callback must retain the browser until it returns"
        );
        assert_eq!(count.get(), 0);
        dispatch_one();
        assert!(weak.upgrade().is_none());
        assert_eq!(count.get(), 1);
    }

    #[test]
    fn cancelled_callback_and_failed_request_also_defer_the_last_reference() {
        initialize().unwrap();
        let count = Rc::new(Cell::new(0));
        let guard = DeferredDrop::new(Rc::new(Probe(count.clone(), thread::current().id())));
        let callback = move || drop(guard);
        // COM 可以在未调用回调的情况下释放它。
        drop(callback);
        let failed_request = || -> Result<(), ()> {
            let _guard = DeferredDrop::new(Rc::new(Probe(count.clone(), thread::current().id())));
            Err(())?;
            Ok(())
        };
        assert!(failed_request().is_err());
        assert_eq!(count.get(), 0);
        dispatch_one();
        assert_eq!(count.get(), 2);
        dispatch_one();
        assert_eq!(
            count.get(),
            2,
            "duplicate notifications must not drop twice"
        );
    }

    #[test]
    fn disposal_can_enqueue_more_work_without_borrowing_the_queue() {
        struct Reentrant(Probe);
        impl Drop for Reentrant {
            fn drop(&mut self) {
                defer_drop(Probe(self.0 .0.clone(), self.0 .1));
            }
        }
        initialize().unwrap();
        let count = Rc::new(Cell::new(0));
        defer_drop(Reentrant(Probe(count.clone(), thread::current().id())));
        dispatch_one();
        assert_eq!(count.get(), 1);
        dispatch_one();
        assert_eq!(count.get(), 2);
    }
}
