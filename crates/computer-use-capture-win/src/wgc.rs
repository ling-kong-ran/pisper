//! Windows.Graphics.Capture（WGC）连续镜像流：macOS SCStream 主路径的 Windows 对等。
//!
//! 零强链接约束：WGC 全部经 WinRT `RoGetActivationFactory` 运行时解析（无导入库），
//! D3D11/DXGI 为 Win7+ 就存在的系统 DLL。旧系统（< Win10 1803）激活失败返回 Err，
//! 调用方降级 GDI 轮询——与 macOS 侧 dlopen ScreenCaptureKit 的策略完全对齐。
//!
//! 手写 vtable 的原因：`GraphicsCaptureSession` 的 composable 构造接口
//! `IGraphicsCaptureSessionFactory` 未包含在 windows-rs 生成代码中，只能按
//! WinRT 激活工厂对象的固定 ABI（IUnknown 3 槽 + IInspectable 3 槽后接
//! CreateInstance）手写最小绑定——与 macOS 侧手写 objc2 绑定同一思路。

use std::ffi::c_void;
use std::ptr::null_mut;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use windows::core::{IInspectable, Interface, GUID, HRESULT, HSTRING};
use windows::Foundation::TypedEventHandler;
use windows::Graphics::Capture::{
    Direct3D11CaptureFrame, Direct3D11CaptureFramePool, GraphicsCaptureItem, GraphicsCaptureSession,
};
use windows::Graphics::DirectX::Direct3D11::IDirect3DDevice;
use windows::Graphics::DirectX::DirectXPixelFormat;
use windows::Graphics::SizeInt32;
use windows::Win32::Foundation::{E_POINTER, HMODULE, HWND};
use windows::Win32::Graphics::Direct3D::{D3D_DRIVER_TYPE_HARDWARE, D3D_DRIVER_TYPE_WARP};
use windows::Win32::Graphics::Direct3D11::{
    D3D11CreateDevice, ID3D11Device, ID3D11DeviceContext, ID3D11Texture2D, D3D11_CPU_ACCESS_READ,
    D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_MAPPED_SUBRESOURCE, D3D11_MAP_READ, D3D11_SDK_VERSION,
    D3D11_TEXTURE2D_DESC, D3D11_USAGE_STAGING,
};
use windows::Win32::Graphics::Dxgi::IDXGIDevice;
// CreateDirect3D11DeviceFromDXGIDevice 与 IDirect3DDxgiInterfaceAccess 同属
// WinRT-D3D11 interop 模块（非 Graphics::DirectX）。
use windows::Win32::System::WinRT::Direct3D11::{
    CreateDirect3D11DeviceFromDXGIDevice, IDirect3DDxgiInterfaceAccess,
};
use windows::Win32::System::WinRT::Graphics::Capture::IGraphicsCaptureItemInterop;
use windows::Win32::System::WinRT::RoGetActivationFactory;
use windows::Win32::UI::WindowsAndMessaging::IsWindow;

use crate::{CaptureError, CapturedFrame};

/// 帧回调：Ok(帧) 送编码管线；Err(WindowMissing/Backend) 触发调用方停流。
/// WGC free-threaded 帧池在线程池线程上回调，要求 Send+Sync。
pub type FrameCallback = Arc<dyn Fn(Result<CapturedFrame, CaptureError>) + Send + Sync>;

// ---- IGraphicsCaptureSessionFactory 手写最小绑定 ----

const IID_IGRAPHICS_CAPTURE_SESSION_FACTORY: GUID = GUID::from_values(
    0x7784056a,
    0x67aa,
    0x4d53,
    [0xae, 0x54, 0x10, 0x88, 0xd5, 0xa8, 0xca, 0x21],
);

/// WinRT 激活工厂对象的 ABI：IUnknown(3 槽) + IInspectable(3 槽) + CreateInstance。
/// 前 6 槽只占位对齐偏移，从不调用。
#[repr(C)]
struct SessionFactoryVtbl {
    _prologue: [usize; 6],
    create_instance: unsafe extern "system" fn(
        this: *mut c_void,
        frame_pool: *mut c_void,
        item: *mut c_void,
        base_interface: *mut *mut c_void,
        value: *mut *mut c_void,
    ) -> HRESULT,
}

/// 透明包装激活工厂指针，借用 windows-core 的 QI/引用计数机制。
#[repr(transparent)]
#[derive(Clone)]
struct SessionFactory(windows::core::IUnknown);

unsafe impl Interface for SessionFactory {
    type Vtable = SessionFactoryVtbl;
    const IID: GUID = IID_IGRAPHICS_CAPTURE_SESSION_FACTORY;
}

/// 经 composable factory 构造 GraphicsCaptureSession（windows-rs 未生成 new）。
fn create_session(
    pool: &Direct3D11CaptureFramePool,
    item: &GraphicsCaptureItem,
) -> windows::core::Result<GraphicsCaptureSession> {
    unsafe {
        let class_name = HSTRING::from("Windows.Graphics.Capture.GraphicsCaptureSession");
        // RoGetActivationFactory 运行时解析：旧系统在此失败 → 调用方降级 GDI 轮询。
        let activation: windows::core::IUnknown = RoGetActivationFactory(&class_name)?;
        let factory: SessionFactory = activation.cast()?;
        let mut session_raw: *mut c_void = null_mut();
        // base_interface 传 NULL：非聚合创建（标准 WinRT composable 语义）。
        (factory.vtable().create_instance)(
            factory.as_raw(),
            pool.as_raw(),
            item.as_raw(),
            null_mut(),
            &mut session_raw,
        )
        .ok()?;
        if session_raw.is_null() {
            return Err(windows::core::Error::from_hresult(E_POINTER));
        }
        Ok(GraphicsCaptureSession::from_raw(session_raw))
    }
}

// ---- WGC 流 ----

/// GPU→CPU 拷贝用的 staging 纹理缓存：尺寸变化（窗口 resize）才重建，
/// 避免每帧 CreateTexture2D 的驱动开销。
struct StagingTexture {
    size: SizeInt32,
    texture: ID3D11Texture2D,
}

/// FrameArrived/Closed 回调共享的状态。
struct SharedState {
    d3d_device: ID3D11Device,
    d3d_context: ID3D11DeviceContext,
    callback: FrameCallback,
    staging: Mutex<Option<StagingTexture>>,
    /// Closed 回调只上报一次 WindowMissing（窗口销毁与主动停流可能竞态）。
    reported_closed: AtomicBool,
}

/// 运行中的 WGC 捕获会话。Drop 即停止（解绑事件 → 关闭 session/pool/item），
/// 与 macOS `sc_stream::StreamHandle` 的 Drop 语义一致。
pub struct WgcStream {
    session: GraphicsCaptureSession,
    pool: Direct3D11CaptureFramePool,
    item: GraphicsCaptureItem,
    frame_token: i64,
    closed_token: i64,
    // WinRT 设备与 D3D 设备须与帧池同生命周期：帧池内部持弱引用，
    // 提前释放会复现 macOS 侧「首帧后静默停摆」同类的悬垂回调问题。
    _winrt_device: IDirect3DDevice,
    _d3d_device: ID3D11Device,
    _d3d_context: ID3D11DeviceContext,
}

impl Drop for WgcStream {
    fn drop(&mut self) {
        // 逆序停止：先解绑回调（阻止新帧进入），再关 session/pool。
        // 全部 best-effort：窗口已销毁/进程收尾时这些调用可能失败，
        // Drop 不允许 panic，失败仅意味着系统已自行回收。
        let _ = self.pool.RemoveFrameArrived(self.frame_token);
        let _ = self.item.RemoveClosed(self.closed_token);
        let _ = self.session.Close();
        let _ = self.pool.Close();
    }
}

/// WGC 是否可用（Win10 1903+ 的 IsSupported 静态方法；更老系统直接 false）。
pub fn wgc_supported() -> bool {
    GraphicsCaptureSession::IsSupported().unwrap_or(false)
}

/// 启动 WGC 镜像流：成功返回句柄（Drop 停止），失败返回 Err（调用方降级轮询）。
pub fn start_wgc(window_id: u64, callback: FrameCallback) -> Result<WgcStream, String> {
    unsafe {
        let hwnd = HWND(window_id as isize as *mut c_void);
        if !IsWindow(Some(hwnd)).as_bool() {
            return Err("target window does not exist (IsWindow=false)".into());
        }

        // D3D11 设备：硬件优先，失败退 WARP 软件光栅（虚拟机/驱动异常仍可镜像）。
        // BGRA_SUPPORT 是 WGC 帧池格式的硬要求。
        let (d3d_device, d3d_context) = create_d3d_device()
            .map_err(|error| format!("D3D11 device creation failed: {error}"))?;

        // WGC 需要 WinRT 包装的 IDirect3DDevice（经 DXGI 设备转换）。
        let dxgi_device: IDXGIDevice = d3d_device
            .cast()
            .map_err(|error| format!("IDXGIDevice cast failed: {error}"))?;
        let inspectable = CreateDirect3D11DeviceFromDXGIDevice(&dxgi_device)
            .map_err(|error| format!("Direct3D11 device wrapper failed: {error}"))?;
        let winrt_device: IDirect3DDevice = inspectable
            .cast()
            .map_err(|error| format!("IDirect3DDevice cast failed: {error}"))?;

        // 非交互创建捕获目标：激活工厂 QI 到 interop 接口（无需 Picker UI）。
        let class_name = HSTRING::from("Windows.Graphics.Capture.GraphicsCaptureItem");
        let activation: windows::core::IUnknown = RoGetActivationFactory(&class_name)
            .map_err(|error| format!("WGC unavailable (Windows 10 1803+ required): {error}"))?;
        let interop: IGraphicsCaptureItemInterop = activation
            .cast()
            .map_err(|error| format!("IGraphicsCaptureItemInterop unavailable: {error}"))?;
        let item: GraphicsCaptureItem = interop
            .CreateForWindow(hwnd)
            .map_err(|error| format!("CreateForWindow failed: {error}"))?;
        let size = item
            .Size()
            .map_err(|error| format!("GraphicsCaptureItem.Size failed: {error}"))?;

        // free-threaded 帧池：回调在线程池触发，无需绑定 UI 线程的
        // DispatcherQueue——镜像监督线程在哪个线程起流都能工作。
        let pool = Direct3D11CaptureFramePool::CreateFreeThreaded(
            &winrt_device,
            DirectXPixelFormat::B8G8R8A8UIntNormalized,
            2,
            size,
        )
        .map_err(|error| format!("frame pool creation failed: {error}"))?;
        let session = create_session(&pool, &item)
            .map_err(|error| format!("capture session creation failed: {error}"))?;

        // 观感对齐 macOS：Win10 2004+ 去掉黄色高亮边框；Win11+ 隐藏光标。
        // 均 best-effort（内部 cast 到对应版本接口）：旧系统失败忽略即可。
        let _ = session.SetIsBorderRequired(false);
        let _ = session.SetIsCursorCaptureEnabled(false);

        let state = Arc::new(SharedState {
            d3d_device: d3d_device.clone(),
            d3d_context: d3d_context.clone(),
            callback,
            staging: Mutex::new(None),
            reported_closed: AtomicBool::new(false),
        });

        // FrameArrived：clone 帧池进闭包（WinRT 包装是引用计数指针，clone 廉价），
        // 不依赖 sender 参数，避开 Ref<T> 生命周期纠缠。
        let frame_state = Arc::clone(&state);
        // WinRT 包装是引用计数指针，Clone 直接得到共享句柄（非 Result）。
        let frame_pool = pool.clone();
        let frame_handler = TypedEventHandler::<Direct3D11CaptureFramePool, IInspectable>::new(
            move |_sender, _args| {
                on_frame_arrived(&frame_pool, &frame_state);
                Ok(())
            },
        );
        let frame_token = pool
            .FrameArrived(&frame_handler)
            .map_err(|error| format!("FrameArrived hook failed: {error}"))?;

        // 窗口销毁 → 上报 WindowMissing（对齐 macOS 的窗口消失监督语义）。
        let closed_state = Arc::clone(&state);
        let closed_handler =
            TypedEventHandler::<GraphicsCaptureItem, IInspectable>::new(move |_sender, _args| {
                if !closed_state.reported_closed.swap(true, Ordering::SeqCst) {
                    (closed_state.callback)(Err(CaptureError::WindowMissing));
                }
                Ok(())
            });
        let closed_token = item
            .Closed(&closed_handler)
            .map_err(|error| format!("Closed hook failed: {error}"))?;

        session
            .StartCapture()
            .map_err(|error| format!("StartCapture failed: {error}"))?;

        Ok(WgcStream {
            session,
            pool,
            item,
            frame_token,
            closed_token,
            _winrt_device: winrt_device,
            _d3d_device: d3d_device,
            _d3d_context: d3d_context,
        })
    }
}

/// 硬件优先、WARP 兜底的 D3D11 设备创建。
unsafe fn create_d3d_device() -> Result<(ID3D11Device, ID3D11DeviceContext), String> {
    for driver in [D3D_DRIVER_TYPE_HARDWARE, D3D_DRIVER_TYPE_WARP] {
        let mut device: Option<ID3D11Device> = None;
        let mut context: Option<ID3D11DeviceContext> = None;
        // SAFETY: out 参数为有效的本地 Option 指针；feature levels 传 None 用默认全集。
        let result = D3D11CreateDevice(
            None,
            driver,
            HMODULE(null_mut()),
            D3D11_CREATE_DEVICE_BGRA_SUPPORT,
            None,
            D3D11_SDK_VERSION,
            Some(&mut device),
            None,
            Some(&mut context),
        );
        if result.is_ok() {
            if let (Some(device), Some(context)) = (device, context) {
                return Ok((device, context));
            }
        }
    }
    Err("neither hardware nor WARP D3D11 device available".into())
}

/// FrameArrived 处理：drain 所有积压帧（帧池可能一次事件带多帧），逐帧拷贝下发；
/// 下游是单槽邮箱（只留最新），积压帧自然被覆盖，不产生排队延迟。
fn on_frame_arrived(pool: &Direct3D11CaptureFramePool, state: &Arc<SharedState>) {
    loop {
        let frame = match pool.TryGetNextFrame() {
            Ok(frame) => frame,
            // 无新帧（E_PENDING）与其他错误都结束本轮 drain。
            Err(_) => return,
        };
        // SAFETY: frame/state 均为本函数栈上有效引用；内部只访问 D3D 接口与
        // Map 返回的暂存纹理内存，拷贝在 Unmap 之前完成。
        let outcome = unsafe { copy_frame_to_cpu(&frame, state) };
        let _ = frame.Close();
        match outcome {
            Ok(Some(captured)) => (state.callback)(Ok(captured)),
            // 0 尺寸帧（窗口最小化瞬间）：跳过，不是错误。
            Ok(None) => {}
            Err(error) => {
                (state.callback)(Err(error));
                return;
            }
        }
    }
}

/// WGC 帧是 GPU 纹理，CPU 不可直接 Map：CopyResource 到 staging 纹理再读出。
unsafe fn copy_frame_to_cpu(
    frame: &Direct3D11CaptureFrame,
    state: &Arc<SharedState>,
) -> Result<Option<CapturedFrame>, CaptureError> {
    let size = frame
        .ContentSize()
        .map_err(|error| CaptureError::Backend(format!("ContentSize: {error}")))?;
    if size.Width <= 0 || size.Height <= 0 {
        return Ok(None);
    }
    let surface = frame
        .Surface()
        .map_err(|error| CaptureError::Backend(format!("Surface: {error}")))?;
    let access: IDirect3DDxgiInterfaceAccess = surface
        .cast()
        .map_err(|error| CaptureError::Backend(format!("surface cast: {error}")))?;
    let source: ID3D11Texture2D = access
        .GetInterface()
        .map_err(|error| CaptureError::Backend(format!("GetInterface: {error}")))?;

    let mut staging_guard = state.staging.lock().unwrap_or_else(|p| p.into_inner());
    let need_new = match &*staging_guard {
        Some(staging) => staging.size.Width != size.Width || staging.size.Height != size.Height,
        None => true,
    };
    if need_new {
        let mut desc = D3D11_TEXTURE2D_DESC::default();
        source.GetDesc(&mut desc);
        desc.Usage = D3D11_USAGE_STAGING;
        desc.BindFlags = 0;
        desc.CPUAccessFlags = D3D11_CPU_ACCESS_READ.0 as u32;
        desc.MiscFlags = 0;
        let mut staging: Option<ID3D11Texture2D> = None;
        state
            .d3d_device
            .CreateTexture2D(&desc, None, Some(&mut staging))
            .map_err(|error| CaptureError::Backend(format!("staging texture: {error}")))?;
        let staging =
            staging.ok_or_else(|| CaptureError::Backend("staging texture missing".into()))?;
        *staging_guard = Some(StagingTexture {
            size,
            texture: staging,
        });
    }
    let staging_texture = &staging_guard
        .as_ref()
        .ok_or_else(|| CaptureError::Backend("staging texture missing".into()))?
        .texture;

    state.d3d_context.CopyResource(staging_texture, &source);
    let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
    state
        .d3d_context
        .Map(staging_texture, 0, D3D11_MAP_READ, 0, Some(&mut mapped))
        .map_err(|error| CaptureError::Backend(format!("map staging: {error}")))?;
    // RowPitch 可能大于 width*4（驱动对齐），必须逐行拷贝。
    let row_bytes = (size.Width as usize) * 4;
    let mut bgra = vec![0u8; row_bytes * size.Height as usize];
    let base = mapped.pData as *const u8;
    for y in 0..size.Height as usize {
        std::ptr::copy_nonoverlapping(
            base.add(y * mapped.RowPitch as usize),
            bgra.as_mut_ptr().add(y * row_bytes),
            row_bytes,
        );
    }
    state.d3d_context.Unmap(staging_texture, 0);
    drop(staging_guard);

    Ok(Some(CapturedFrame {
        width: size.Width as u32,
        height: size.Height as u32,
        bgra,
    }))
}
