import AVFoundation
import Contacts
import CoreLocation
import Darwin
import Foundation
import Network
import Photos
import UserNotifications
import Tauri
import UIKit
import WebKit

struct PermissionArgs: Decodable {
  let capability: String
}

struct OperationParameters: Decodable {
  let query: String?
  let limit: Int?
  let cameraDirection: String?
  let url: String?
  let latitude: Double?
  let longitude: Double?
  let label: String?
  let phoneNumber: String?
  let message: String?
  let text: String?
  let body: String?
  let title: String?
  let notifyId: String?
  let packageName: String?
  let appUrl: String?
  let intensity: String?
  let durationMs: Int?
  let enabled: Bool?
  let albumId: String?
  let albumName: String?
  let assetIds: [String]?
  let confirmed: Bool?
  let mediaType: String?
  let fromDate: String?
  let toDate: String?
  let fileName: String?
  let mimeType: String?
  let data: String?
}

struct OperationArgs: Decodable {
  let operation: String
  let parameters: OperationParameters?
}

struct ContactItem: Encodable {
  let id: String
  let name: String
  let phones: [String]
}

struct ContactResult: Encodable {
  let contacts: [ContactItem]
  let count: Int
  let limited: Bool
}

struct PhotoResult: Encodable {
  let data: String
  let mimeType: String
  let width: Int
  let height: Int
}

struct LocationResult: Encodable {
  let latitude: Double
  let longitude: Double
  let accuracyMeters: Double
  let altitudeMeters: Double?
  let timestamp: Int
}

struct ExternalOpenResult: Encodable {
  let opened: Bool
  let operation: String
  let target: String
}

struct DeviceInfoResult: Encodable {
  let platform: String
  let manufacturer: String
  let model: String
  let device: String
  let systemVersion: String
  let sdkInt: Int
  let appVersion: String
  let processorCount: Int
  let memory: [String: UInt64]
}

struct BatteryResult: Encodable {
  let level: Float
  let status: String
  let lowPowerMode: Bool
}

struct StorageResult: Encodable {
  let totalBytes: UInt64
  let availableBytes: UInt64
  let usedBytes: UInt64
  let scope: String
}

struct MemoryStatusResult: Encodable {
  let totalBytes: UInt64
  let availableBytes: UInt64?
  let processAvailableBytes: UInt64?
  let appUsedBytes: UInt64?
  let lowMemory: Bool?
  let scope: String
  let limitations: [String]
}

struct NetworkStatusResult: Encodable {
  let connected: Bool
  let transport: String
  let interfaces: [String]
  let metered: Bool
  let constrained: Bool
  let validated: Bool?
}

struct DisplayStatusResult: Encodable {
  let widthPixels: Int
  let heightPixels: Int
  let scale: Double
  let orientation: String
  let brightness: Float?
}

struct LocaleStatusResult: Encodable {
  let languageTag: String
  let languageCode: String
  let regionCode: String
  let calendar: String
  let timeZone: String
}

struct DeviceStatusResult: Encodable {
  let device: DeviceInfoResult
  let memory: MemoryStatusResult
  let storage: StorageResult
  let battery: BatteryResult
  let network: NetworkStatusResult
  let display: DisplayStatusResult
  let locale: LocaleStatusResult
  let platformLimitations: [String]
}

struct DeviceCapability: Encodable {
  let action: String
  let operation: String
  let available: Bool
  let permission: String
  let permissionState: String
  let requiredParameters: [String]
  let optionalParameters: [String]
  let limitations: [String]
}

struct CapabilitiesResult: Encodable {
  let platform: String
  let systemVersion: String
  let sdkInt: Int
  let appVersion: String
  let capabilities: [DeviceCapability]
  let platformLimitations: [String]
}

struct PhotoAssetResult: Encodable {
  let id: String
  let filename: String
  let mediaType: String
  let width: Int
  let height: Int
  let durationSeconds: Double
  let createdAt: String?
  let modifiedAt: String?
  let latitude: Double?
  let longitude: Double?
  let favorite: Bool
  let hidden: Bool
}

struct PhotoListResult: Encodable {
  let photos: [PhotoAssetResult]
  let count: Int
  let limited: Bool
}

final class MobileDevicePlugin: Plugin, UIImagePickerControllerDelegate,
  UINavigationControllerDelegate, CLLocationManagerDelegate
{
  private let contactStore = CNContactStore()
  private let locationManager = CLLocationManager()
  private var permissionInvoke: Invoke?
  private var locationInvoke: Invoke?
  private var cameraInvoke: Invoke?
  private weak var webView: WKWebView?
  private var keyboardObservers: [NSObjectProtocol] = []

  override init() {
    super.init()
    locationManager.delegate = self
    UIDevice.current.isBatteryMonitoringEnabled = true
  }

  override func load(webview: WKWebView) {
    self.webView = webview
    let center = NotificationCenter.default
    let names: [(Notification.Name, Bool)] = [
      (UIResponder.keyboardDidShowNotification, true),
      (UIResponder.keyboardDidChangeFrameNotification, true),
      (UIResponder.keyboardDidHideNotification, false),
    ]
    keyboardObservers = names.map { name, open in
      center.addObserver(forName: name, object: nil, queue: .main) { [weak self] _ in
        guard let webView = self?.webView else { return }
        let state = open ? "open" : "closed"
        webView.evaluateJavaScript(
          "document.documentElement.dataset.mobileKeyboard = '\(state)'"
        )
      }
    }
  }

  deinit {
    for observer in keyboardObservers {
      NotificationCenter.default.removeObserver(observer)
    }
  }

  private func authorizationState(_ capability: String) -> String {
    switch capability {
    case "contacts":
      let status = CNContactStore.authorizationStatus(for: .contacts)
      switch status {
      case .authorized: return "granted"
      case .denied, .restricted: return "denied"
      case .notDetermined: return "prompt"
      default:
        // iOS 18 的 limited 联系人状态只能通过 rawValue 兼容旧 SDK 识别。
        if #available(iOS 18.0, *), status.rawValue == 4 { return "granted" }
        return "prompt"
      }
    case "camera":
      switch AVCaptureDevice.authorizationStatus(for: .video) {
      case .authorized: return "granted"
      case .denied, .restricted: return "denied"
      case .notDetermined: return "prompt"
      @unknown default: return "prompt"
      }
    case "microphone":
      switch AVAudioSession.sharedInstance().recordPermission {
      case .granted: return "granted"
      case .denied: return "denied"
      case .undetermined: return "prompt"
      @unknown default: return "prompt"
      }
    case "location":
      switch locationManager.authorizationStatus {
      case .authorizedWhenInUse, .authorizedAlways: return "granted"
      case .denied, .restricted: return "denied"
      case .notDetermined: return "prompt"
      @unknown default: return "prompt"
      }
    case "photos":
      switch PHPhotoLibrary.authorizationStatus(for: .readWrite) {
      case .authorized: return "granted"
      case .limited: return "limited"
      case .denied, .restricted: return "denied"
      case .notDetermined: return "prompt"
      @unknown default: return "prompt"
      }
    case "notifications": return "prompt"
    case "externalApps": return "not-required"
    default:
      return "unsupported"
    }
  }

  private func notificationState(_ completion: @escaping (String) -> Void) {
    UNUserNotificationCenter.current().getNotificationSettings { settings in
      let state: String
      switch settings.authorizationStatus {
      case .authorized, .provisional, .ephemeral: state = "granted"
      case .denied: state = "denied"
      case .notDetermined: state = "prompt"
      @unknown default: state = "prompt"
      }
      completion(state)
    }
  }

  @objc public func permissionStates(_ invoke: Invoke) {
    notificationState { state in
      invoke.resolve([
        "contacts": self.authorizationState("contacts"),
        "camera": self.authorizationState("camera"),
        "microphone": self.authorizationState("microphone"),
        "location": self.authorizationState("location"),
        "photos": self.authorizationState("photos"),
        "notifications": state,
        "externalApps": self.authorizationState("externalApps"),
      ])
    }
  }

  @objc public func requestPermission(_ invoke: Invoke) throws {
    let args = try invoke.parseArgs(PermissionArgs.self)
    switch args.capability {
    case "contacts":
      contactStore.requestAccess(for: .contacts) { _, error in
        if let error = error {
          invoke.reject(error.localizedDescription)
        } else {
          invoke.resolve([
            "capability": "contacts",
            "state": self.authorizationState("contacts"),
          ])
        }
      }
    case "camera":
      AVCaptureDevice.requestAccess(for: .video) { _ in
        invoke.resolve([
          "capability": "camera",
          "state": self.authorizationState("camera"),
        ])
      }
    case "microphone":
      AVAudioSession.sharedInstance().requestRecordPermission { _ in
        invoke.resolve([
          "capability": "microphone",
          "state": self.authorizationState("microphone"),
        ])
      }
    case "location":
      if authorizationState("location") == "prompt" {
        permissionInvoke = invoke
        DispatchQueue.main.async { self.locationManager.requestWhenInUseAuthorization() }
      } else {
        invoke.resolve([
          "capability": "location",
          "state": authorizationState("location"),
        ])
      }
    case "photos":
      if authorizationState("photos") == "prompt" {
        PHPhotoLibrary.requestAuthorization(for: .readWrite) { _ in
          invoke.resolve([
            "capability": "photos",
            "state": self.authorizationState("photos"),
          ])
        }
      } else {
        invoke.resolve([
          "capability": "photos",
          "state": authorizationState("photos"),
        ])
      }
    case "notifications":
      notificationState { state in
        if state == "prompt" {
          UNUserNotificationCenter.current().requestAuthorization(options: [.alert, .sound, .badge]) { _, error in
            if let error {
              invoke.reject(error.localizedDescription)
            } else {
              self.notificationState { updated in
                invoke.resolve(["capability": "notifications", "state": updated])
              }
            }
          }
        } else {
          invoke.resolve(["capability": "notifications", "state": state])
        }
      }
    case "externalApps":
      invoke.resolve([
        "capability": "externalApps",
        "state": authorizationState("externalApps"),
      ])
    default:
      invoke.reject("Unsupported mobile capability: \(args.capability)")
    }
  }

  @objc public func openAppSettings(_ invoke: Invoke) {
    guard let url = URL(string: UIApplication.openSettingsURLString) else {
      invoke.reject("Unable to open App settings")
      return
    }
    DispatchQueue.main.async {
      UIApplication.shared.open(url, options: [:]) { _ in invoke.resolve() }
    }
  }

  @objc public func execute(_ invoke: Invoke) throws {
    let args = try invoke.parseArgs(OperationArgs.self)
    switch args.operation {
    case "contacts.search": searchContacts(invoke, parameters: args.parameters)
    case "camera.capture": capturePhoto(invoke, parameters: args.parameters)
    case "location.current": currentLocation(invoke)
    case "device.info": deviceInfo(invoke)
    case "device.capabilities": capabilities(invoke)
    case "device.battery": batteryStatus(invoke)
    case "device.storage": storageStatus(invoke)
    case "device.memory": memoryStatus(invoke)
    case "device.network": networkStatus(invoke)
    case "device.display": displayStatus(invoke)
    case "device.locale": localeStatus(invoke)
    case "device.status": deviceStatus(invoke)
    case "device.clipboard.get": getClipboard(invoke)
    case "device.clipboard.set": setClipboard(invoke, parameters: args.parameters)
    case "device.vibrate": vibrate(invoke, parameters: args.parameters)
    case "device.notify": sendNotification(invoke, parameters: args.parameters)
    case "device.flashlight": setFlashlight(invoke, parameters: args.parameters)
    case "photos.list": listPhotos(invoke, parameters: args.parameters)
    case "photos.create_album": createPhotoAlbum(invoke, parameters: args.parameters)
    case "photos.add_to_album": addPhotosToAlbum(invoke, parameters: args.parameters)
    case "photos.delete": deletePhotos(invoke, parameters: args.parameters)
    case "apps.share_text": shareText(invoke, parameters: args.parameters)
    case "apps.open_url": openURL(invoke, parameters: args.parameters)
    case "apps.open_map": openMap(invoke, parameters: args.parameters)
    case "apps.open_system_settings": openSystemSettings(invoke)
    case "apps.open_dialer": openDialer(invoke, parameters: args.parameters)
    case "apps.compose_sms": composeSMS(invoke, parameters: args.parameters)
    case "apps.open_app": openApp(invoke, parameters: args.parameters)
    case "files.open": openFile(invoke, parameters: args.parameters)
    default: invoke.reject("Unsupported mobile operation: \(args.operation)")
    }
  }

  private func openExternalURL(
    _ invoke: Invoke, url: URL, operation: String, target: String
  ) {
    DispatchQueue.main.async {
      UIApplication.shared.open(url, options: [:]) { opened in
        if opened {
          invoke.resolve(ExternalOpenResult(opened: true, operation: operation, target: target))
        } else {
          invoke.reject("No compatible application is installed")
        }
      }
    }
  }

  private func openURL(_ invoke: Invoke, parameters: OperationParameters?) {
    let value = parameters?.url?.trimmingCharacters(in: .whitespacesAndNewlines) ?? ""
    guard value.count <= 2_048, let components = URLComponents(string: value),
      ["http", "https"].contains(components.scheme?.lowercased() ?? ""),
      components.host?.isEmpty == false, let url = components.url
    else {
      invoke.reject("Only valid HTTP or HTTPS URLs can be opened")
      return
    }
    openExternalURL(invoke, url: url, operation: "apps.open_url", target: url.absoluteString)
  }

  private func openMap(_ invoke: Invoke, parameters: OperationParameters?) {
    guard let latitude = parameters?.latitude, (-90...90).contains(latitude),
      let longitude = parameters?.longitude, (-180...180).contains(longitude)
    else {
      invoke.reject("Map coordinates are invalid")
      return
    }
    let label = parameters?.label?.trimmingCharacters(in: .whitespacesAndNewlines) ?? ""
    guard label.count <= 120 else {
      invoke.reject("Map label is too long")
      return
    }
    var components = URLComponents()
    components.scheme = "https"
    components.host = "maps.apple.com"
    components.path = "/"
    components.queryItems = [
      URLQueryItem(name: "ll", value: "\(latitude),\(longitude)"),
      URLQueryItem(name: "q", value: label.isEmpty ? "\(latitude),\(longitude)" : label),
    ]
    guard let url = components.url else {
      invoke.reject("Unable to create the map URL")
      return
    }
    openExternalURL(
      invoke, url: url, operation: "apps.open_map", target: "\(latitude),\(longitude)")
  }

  private func openSystemSettings(_ invoke: Invoke) {
    guard let url = URL(string: UIApplication.openSettingsURLString) else {
      invoke.reject("Unable to open App settings")
      return
    }
    openExternalURL(invoke, url: url, operation: "apps.open_system_settings", target: "app")
  }

  private func validPhoneNumber(_ value: String) -> Bool {
    value.count <= 64
      && value.range(of: #"^[+*#0-9(). \-]{1,64}$"#, options: .regularExpression) != nil
  }

  private func openDialer(_ invoke: Invoke, parameters: OperationParameters?) {
    let number = parameters?.phoneNumber?.trimmingCharacters(in: .whitespacesAndNewlines) ?? ""
    guard validPhoneNumber(number),
      let encoded = number.addingPercentEncoding(
        withAllowedCharacters: CharacterSet(charactersIn: "+*0123456789")),
      let url = URL(string: "tel:\(encoded)")
    else {
      invoke.reject("Phone number is invalid")
      return
    }
    openExternalURL(invoke, url: url, operation: "apps.open_dialer", target: number)
  }

  private func composeSMS(_ invoke: Invoke, parameters: OperationParameters?) {
    let number = parameters?.phoneNumber?.trimmingCharacters(in: .whitespacesAndNewlines) ?? ""
    let message = parameters?.message ?? ""
    guard validPhoneNumber(number), message.count <= 2_000 else {
      invoke.reject("SMS draft parameters are invalid")
      return
    }
    var components = URLComponents()
    components.scheme = "sms"
    components.path = number
    if !message.isEmpty { components.queryItems = [URLQueryItem(name: "body", value: message)] }
    guard let url = components.url else {
      invoke.reject("Unable to create the SMS draft")
      return
    }
    openExternalURL(invoke, url: url, operation: "apps.compose_sms", target: number)
  }

  private func openApp(_ invoke: Invoke, parameters: OperationParameters?) {
    let value = parameters?.appUrl?.trimmingCharacters(in: .whitespacesAndNewlines) ?? ""
    let forbidden = Set(["about", "content", "data", "file", "http", "https", "intent", "javascript"])
    guard value.count <= 2_048, let url = URL(string: value),
      let scheme = url.scheme?.lowercased(), !scheme.isEmpty, !forbidden.contains(scheme)
    else {
      invoke.reject("A valid third-party application URL scheme is required on iOS")
      return
    }
    openExternalURL(invoke, url: url, operation: "apps.open_app", target: scheme)
  }

  private func requirePermission(_ capability: String, invoke: Invoke) -> Bool {
    guard authorizationState(capability) == "granted" else {
      invoke.reject("Pisper does not have \(capability) permission")
      return false
    }
    return true
  }

  private func requirePhotosPermission(_ invoke: Invoke) -> Bool {
    let state = authorizationState("photos")
    guard state == "granted" || state == "limited" else {
      invoke.reject("Pisper does not have photos permission")
      return false
    }
    return true
  }

  private func processMemoryBytes() -> UInt64? {
    var info = task_vm_info_data_t()
    var count = mach_msg_type_number_t(
      MemoryLayout<task_vm_info_data_t>.stride / MemoryLayout<integer_t>.stride
    )
    let result = withUnsafeMutablePointer(to: &info) { pointer in
      pointer.withMemoryRebound(to: integer_t.self, capacity: Int(count)) { rebound in
        task_info(
          mach_task_self_,
          task_flavor_t(TASK_VM_INFO),
          rebound,
          &count
        )
      }
    }
    guard result == KERN_SUCCESS else { return nil }
    return UInt64(info.phys_footprint)
  }

  private func deviceInfoResult() -> DeviceInfoResult {
    var memory = ["totalBytes": ProcessInfo.processInfo.physicalMemory]
    if let appUsedBytes = processMemoryBytes() { memory["appUsedBytes"] = appUsedBytes }
    return DeviceInfoResult(
      platform: "ios",
      manufacturer: "Apple",
      model: UIDevice.current.model,
      device: UIDevice.current.name,
      systemVersion: UIDevice.current.systemVersion,
      sdkInt: Int(UIDevice.current.systemVersion.split(separator: ".").first ?? "0") ?? 0,
      appVersion: Bundle.main.object(forInfoDictionaryKey: "CFBundleShortVersionString") as? String ?? "",
      processorCount: ProcessInfo.processInfo.activeProcessorCount,
      memory: memory
    )
  }

  private func deviceInfo(_ invoke: Invoke) {
    invoke.resolve(deviceInfoResult())
  }

  private func memoryStatusResult() -> MemoryStatusResult {
    MemoryStatusResult(
      totalBytes: ProcessInfo.processInfo.physicalMemory,
      availableBytes: nil,
      processAvailableBytes: UInt64(os_proc_available_memory()),
      appUsedBytes: processMemoryBytes(),
      lowMemory: nil,
      scope: "physical_memory",
      limitations: ["iOS 不提供全局可用系统内存；availableBytes 不可用。"]
    )
  }

  private func memoryStatus(_ invoke: Invoke) {
    invoke.resolve(memoryStatusResult())
  }

  private func capability(
    action: String,
    operation: String,
    available: Bool = true,
    permission: String = "none",
    permissionState: String = "not-required",
    requiredParameters: [String] = [],
    optionalParameters: [String] = [],
    limitations: [String] = []
  ) -> DeviceCapability {
    DeviceCapability(
      action: action,
      operation: operation,
      available: available,
      permission: permission,
      permissionState: permissionState,
      requiredParameters: requiredParameters,
      optionalParameters: optionalParameters,
      limitations: limitations
    )
  }

  private func capabilities(_ invoke: Invoke) {
    notificationState { [weak self] notificationsState in
      guard let self = self else { return }
      invoke.resolve(self.capabilitiesResult(notificationsState: notificationsState))
    }
  }

  private func capabilitiesResult(notificationsState: String) -> CapabilitiesResult {
    let systemVersion = UIDevice.current.systemVersion
    let sdkInt = Int(systemVersion.split(separator: ".").first ?? "0") ?? 0
    let appVersion = Bundle.main.object(forInfoDictionaryKey: "CFBundleShortVersionString") as? String ?? ""
    let cameraAvailable = UIImagePickerController.isSourceTypeAvailable(.camera)
    let flashlightAvailable = AVCaptureDevice.default(for: .video)?.hasTorch == true
    let locationAvailable = CLLocationManager.locationServicesEnabled()
    return CapabilitiesResult(
      platform: "ios",
      systemVersion: systemVersion,
      sdkInt: sdkInt,
      appVersion: appVersion,
      capabilities: [
        capability(
          action: "search_contacts",
          operation: "contacts.search",
          permission: "contacts",
          permissionState: authorizationState("contacts"),
          optionalParameters: ["query", "limit"]
        ),
        capability(
          action: "capture_photo",
          operation: "camera.capture",
          available: cameraAvailable,
          permission: "camera",
          permissionState: authorizationState("camera"),
          optionalParameters: ["cameraDirection"],
          limitations: ["需要在前台显示系统相机界面。"]
        ),
        capability(
          action: "record_audio",
          operation: "audio.record",
          permission: "microphone",
          permissionState: authorizationState("microphone"),
          limitations: ["仅在用户主动点击语音输入后采集。"]
        ),
        capability(
          action: "get_location",
          operation: "location.current",
          available: locationAvailable,
          permission: "location",
          permissionState: authorizationState("location"),
          limitations: ["定位服务关闭时不可用。"]
        ),
        capability(action: "get_device_info", operation: "device.info"),
        capability(action: "get_battery_status", operation: "device.battery"),
        capability(
          action: "get_storage_status",
          operation: "device.storage",
          limitations: ["返回当前设备存储卷的总容量、可用容量和已用容量。"]
        ),
        capability(
          action: "get_memory_status",
          operation: "device.memory",
          limitations: ["iOS 不提供全局可用系统内存；processAvailableBytes 仅表示当前 App 在自身内存上限前的估算余量。"]
        ),
        capability(action: "get_network_status", operation: "device.network"),
        capability(action: "get_display_status", operation: "device.display"),
        capability(action: "get_locale_status", operation: "device.locale"),
        capability(
          action: "get_device_status",
          operation: "device.status",
          limitations: ["包含上述只读设备状态；其中内存限制遵循 get_memory_status 的说明。"]
        ),
        capability(
          action: "get_clipboard",
          operation: "device.clipboard.get",
          limitations: ["系统可能显示剪贴板隐私提示。"]
        ),
        capability(
          action: "set_clipboard",
          operation: "device.clipboard.set",
          requiredParameters: ["text"]
        ),
        capability(
          action: "vibrate",
          operation: "device.vibrate",
          optionalParameters: ["intensity"],
          limitations: ["当前 iOS 实现只使用 intensity。"]
        ),
        capability(
          action: "set_flashlight",
          operation: "device.flashlight",
          available: flashlightAvailable,
          permission: "camera",
          permissionState: authorizationState("camera"),
          requiredParameters: ["enabled"]
        ),
        capability(
          action: "send_notification",
          operation: "device.notify",
          permission: "notifications",
          permissionState: notificationsState,
          requiredParameters: ["body"],
          optionalParameters: ["title", "notifyId"]
        ),
        capability(
          action: "list_photos",
          operation: "photos.list",
          permission: "photos",
          permissionState: authorizationState("photos"),
          optionalParameters: ["limit", "mediaType", "fromDate", "toDate"]
        ),
        capability(
          action: "create_photo_album",
          operation: "photos.create_album",
          permission: "photos",
          permissionState: authorizationState("photos"),
          requiredParameters: ["albumName"]
        ),
        capability(
          action: "add_photos_to_album",
          operation: "photos.add_to_album",
          permission: "photos",
          permissionState: authorizationState("photos"),
          requiredParameters: ["albumId", "assetIds", "confirmed"],
          limitations: ["只能操作已知照片 ID，且必须明确确认。"]
        ),
        capability(
          action: "delete_photos",
          operation: "photos.delete",
          permission: "photos",
          permissionState: authorizationState("photos"),
          requiredParameters: ["assetIds", "confirmed"],
          limitations: ["必须明确确认删除。"]
        ),
        capability(
          action: "share_text",
          operation: "apps.share_text",
          requiredParameters: ["text"],
          limitations: ["打开系统分享面板不代表内容已经分享。"]
        ),
        capability(
          action: "open_url",
          operation: "apps.open_url",
          requiredParameters: ["url"],
          limitations: ["只接受 HTTP 或 HTTPS URL。"]
        ),
        capability(
          action: "open_map",
          operation: "apps.open_map",
          requiredParameters: ["latitude", "longitude"],
          optionalParameters: ["label"]
        ),
        capability(action: "open_system_settings", operation: "apps.open_system_settings"),
        capability(
          action: "open_dialer",
          operation: "apps.open_dialer",
          requiredParameters: ["phoneNumber"],
          limitations: ["只打开预填号码的拨号界面，不会自动拨号。"]
        ),
        capability(
          action: "compose_sms",
          operation: "apps.compose_sms",
          requiredParameters: ["phoneNumber"],
          optionalParameters: ["message"],
          limitations: ["只打开预填短信界面，不会自动发送。"]
        ),
        capability(
          action: "open_app",
          operation: "apps.open_app",
          requiredParameters: ["appUrl"],
          limitations: ["必须使用目标 App 已公开的 URL Scheme，不能使用 Android 包名。"]
        ),
      ],
      platformLimitations: [
        "iOS 第三方 App 不提供全局可用系统内存。",
        "不支持读取短信内容或自动发送短信。",
        "不支持注入或控制第三方 App 的界面。",
      ]
    )
  }

  private func batteryStatusResult() -> BatteryResult {
    let device = UIDevice.current
    return BatteryResult(
      level: device.batteryLevel,
      status: {
        switch device.batteryState {
        case .charging: return "charging"
        case .full: return "full"
        case .unplugged: return "discharging"
        case .unknown: return "unknown"
        @unknown default: return "unknown"
        }
      }(),
      lowPowerMode: ProcessInfo.processInfo.isLowPowerModeEnabled
    )
  }

  private func batteryStatus(_ invoke: Invoke) {
    invoke.resolve(batteryStatusResult())
  }

  private func storageStatusResult() throws -> StorageResult {
    let url = URL(fileURLWithPath: NSHomeDirectory())
    let values = try url.resourceValues(forKeys: [
      .volumeTotalCapacityKey,
      .volumeAvailableCapacityKey,
    ])
    let totalBytes = UInt64(max(values.volumeTotalCapacity ?? 0, 0))
    let availableBytes = UInt64(max(values.volumeAvailableCapacity ?? 0, 0))
    guard totalBytes > 0 else { throw NSError(domain: "PisperMobileDevice", code: 1) }
    let boundedAvailableBytes = min(availableBytes, totalBytes)
    return StorageResult(
      totalBytes: totalBytes,
      availableBytes: boundedAvailableBytes,
      usedBytes: totalBytes - boundedAvailableBytes,
      scope: "device_volume"
    )
  }

  private func storageStatus(_ invoke: Invoke) {
    do {
      invoke.resolve(try storageStatusResult())
    } catch {
      invoke.reject(error.localizedDescription)
    }
  }

  private func orientationName() -> String {
    switch UIDevice.current.orientation {
    case .portrait: return "portrait"
    case .portraitUpsideDown: return "portrait-upside-down"
    case .landscapeLeft: return "landscape-left"
    case .landscapeRight: return "landscape-right"
    case .faceUp: return "face-up"
    case .faceDown: return "face-down"
    case .unknown: return "unknown"
    @unknown default: return "unknown"
    }
  }

  private func displayStatusResult() -> DisplayStatusResult {
    let screen = UIScreen.main
    return DisplayStatusResult(
      widthPixels: Int(screen.nativeBounds.width),
      heightPixels: Int(screen.nativeBounds.height),
      scale: Double(screen.nativeScale),
      orientation: orientationName(),
      brightness: Float(screen.brightness)
    )
  }

  private func displayStatus(_ invoke: Invoke) {
    invoke.resolve(displayStatusResult())
  }

  private func localeStatusResult() -> LocaleStatusResult {
    let locale = Locale.current
    return LocaleStatusResult(
      languageTag: locale.identifier.replacingOccurrences(of: "_", with: "-"),
      languageCode: locale.languageCode ?? "",
      regionCode: locale.regionCode ?? "",
      calendar: String(describing: Calendar.current.identifier),
      timeZone: TimeZone.current.identifier
    )
  }

  private func localeStatus(_ invoke: Invoke) {
    invoke.resolve(localeStatusResult())
  }

  private func deviceStatus(_ invoke: Invoke) {
    networkStatusResult { [weak self] network in
      guard let self = self else { return }
      do {
        invoke.resolve(
          DeviceStatusResult(
            device: self.deviceInfoResult(),
            memory: self.memoryStatusResult(),
            storage: try self.storageStatusResult(),
            battery: self.batteryStatusResult(),
            network: network,
            display: self.displayStatusResult(),
            locale: self.localeStatusResult(),
            platformLimitations: [
              "iOS 第三方 App 不提供全局可用系统内存。",
              "不支持读取短信内容或自动发送短信。",
              "不支持注入或控制第三方 App 的界面。",
            ]
          )
        )
      } catch {
        invoke.reject(error.localizedDescription)
      }
    }
  }

  private func networkStatus(_ invoke: Invoke) {
    networkStatusResult { result in invoke.resolve(result) }
  }

  private func networkStatusResult(_ completion: @escaping (NetworkStatusResult) -> Void) {
    let monitor = NWPathMonitor()
    monitor.pathUpdateHandler = { path in
      let interfaces = path.availableInterfaces.map { interface in
        switch interface.type {
        case .wifi: return "wifi"
        case .cellular: return "cellular"
        case .wiredEthernet: return "wired-ethernet"
        case .loopback: return "loopback"
        case .other: return "other"
        @unknown default: return "unknown"
        }
      }
      let transport: String
      if path.usesInterfaceType(.wifi) {
        transport = "wifi"
      } else if path.usesInterfaceType(.cellular) {
        transport = "cellular"
      } else if path.usesInterfaceType(.wiredEthernet) {
        transport = "wired-ethernet"
      } else if path.usesInterfaceType(.other) {
        transport = "other"
      } else {
        transport = "none"
      }
      monitor.cancel()
      completion(
        NetworkStatusResult(
          connected: path.status == .satisfied,
          transport: path.status == .satisfied ? transport : "none",
          interfaces: interfaces,
          metered: path.isExpensive,
          constrained: path.isConstrained,
          validated: nil
        )
      )
    }
    monitor.start(queue: DispatchQueue.global(qos: .utility))
  }

  private func getClipboard(_ invoke: Invoke) {
    let text = UIPasteboard.general.string
    guard text == nil || text!.count <= 100_000 else {
      invoke.reject("剪贴板文本超过 100000 个字符。")
      return
    }
    invoke.resolve([
      "text": text ?? "",
      "hasText": text != nil,
      "length": text?.count ?? 0,
    ])
  }

  private func setClipboard(_ invoke: Invoke, parameters: OperationParameters?) {
    let text = parameters?.text ?? ""
    guard !text.isEmpty, text.count <= 100_000 else {
      invoke.reject("剪贴板文本不能为空且不能超过 100000 个字符。")
      return
    }
    UIPasteboard.general.string = text
    invoke.resolve(["set": true, "length": text.count])
  }

  private func vibrate(_ invoke: Invoke, parameters: OperationParameters?) {
    let style: UIImpactFeedbackGenerator.FeedbackStyle
    switch parameters?.intensity {
    case "light": style = .light
    case "heavy": style = .heavy
    default: style = .medium
    }
    DispatchQueue.main.async {
      let generator = UIImpactFeedbackGenerator(style: style)
      generator.prepare()
      generator.impactOccurred()
      invoke.resolve(["vibrated": true])
    }
  }

  private func sendNotification(_ invoke: Invoke, parameters: OperationParameters?) {
    let body = parameters?.body?.trimmingCharacters(in: .whitespacesAndNewlines) ?? ""
    guard !body.isEmpty, body.count <= 4_000 else {
      invoke.reject("Notification body is empty or too long")
      return
    }
    notificationState { state in
      guard state == "granted" else {
        invoke.reject("Pisper does not have notifications permission")
        return
      }
      let identifier = parameters?.notifyId?.trimmingCharacters(in: .whitespacesAndNewlines)
      guard identifier == nil || (identifier!.count <= 120 && !identifier!.contains("\n")) else {
        invoke.reject("Notification ID is invalid")
        return
      }
      let content = UNMutableNotificationContent()
      let title = parameters?.title?.trimmingCharacters(in: .whitespacesAndNewlines).prefix(120).description ?? ""
      content.title = title.isEmpty ? "Pisper" : title
      content.body = body
      content.sound = .default
      let request = UNNotificationRequest(
        identifier: identifier?.isEmpty == false ? identifier! : UUID().uuidString,
        content: content,
        trigger: nil
      )
      UNUserNotificationCenter.current().add(request) { error in
        if let error {
          invoke.reject(error.localizedDescription)
        } else {
          invoke.resolve(["posted": true, "notifyId": request.identifier])
        }
      }
    }
  }

  private func setFlashlight(_ invoke: Invoke, parameters: OperationParameters?) {
    guard let device = AVCaptureDevice.default(for: .video), device.hasTorch else {
      invoke.reject("This device does not provide a flashlight")
      return
    }
    let enabled = parameters?.enabled ?? true
    do {
      try device.lockForConfiguration()
      if enabled {
        try device.setTorchModeOn(level: AVCaptureDevice.maxAvailableTorchLevel)
      } else {
        device.torchMode = .off
      }
      device.unlockForConfiguration()
      invoke.resolve(["enabled": enabled])
    } catch {
      invoke.reject(error.localizedDescription)
    }
  }

  private func isoString(_ date: Date?) -> String? {
    guard let date else { return nil }
    return ISO8601DateFormatter().string(from: date)
  }

  private func photoAsset(_ asset: PHAsset) -> PhotoAssetResult {
    let resource = PHAssetResource.assetResources(for: asset).first
    let mediaType: String
    switch asset.mediaType {
    case .image: mediaType = "image"
    case .video: mediaType = "video"
    case .audio: mediaType = "audio"
    case .unknown: mediaType = "unknown"
    @unknown default: mediaType = "unknown"
    }
    return PhotoAssetResult(
      id: asset.localIdentifier,
      filename: resource?.originalFilename ?? "",
      mediaType: mediaType,
      width: asset.pixelWidth,
      height: asset.pixelHeight,
      durationSeconds: asset.duration,
      createdAt: isoString(asset.creationDate),
      modifiedAt: isoString(asset.modificationDate),
      latitude: asset.location?.coordinate.latitude,
      longitude: asset.location?.coordinate.longitude,
      favorite: asset.isFavorite,
      hidden: asset.isHidden
    )
  }

  private func photoAssets(_ ids: [String]) -> [PHAsset] {
    let result = PHAsset.fetchAssets(withLocalIdentifiers: ids, options: nil)
    var assets = [PHAsset]()
    result.enumerateObjects { asset, _, _ in assets.append(asset) }
    return assets
  }

  private func validPhotoIds(_ ids: [String]?) -> [String]? {
    guard let ids, !ids.isEmpty, ids.count <= 100 else { return nil }
    let unique = Array(Set(ids))
    guard unique.count == ids.count, unique.allSatisfy({ !$0.isEmpty && $0.count <= 256 }) else {
      return nil
    }
    return unique
  }

  private func photoCollection(_ id: String?) -> PHAssetCollection? {
    guard let id, !id.isEmpty, id.count <= 256 else { return nil }
    return PHAssetCollection.fetchAssetCollections(withLocalIdentifiers: [id], options: nil).firstObject
  }

  private func parsedDate(_ value: String?) -> Date? {
    guard let value, !value.isEmpty else { return nil }
    return ISO8601DateFormatter().date(from: value)
  }

  private func listPhotos(_ invoke: Invoke, parameters: OperationParameters?) {
    let fromDate = parsedDate(parameters?.fromDate)
    let toDate = parsedDate(parameters?.toDate)
    if (parameters?.fromDate != nil && fromDate == nil) ||
      (parameters?.toDate != nil && toDate == nil) ||
      (fromDate != nil && toDate != nil && fromDate! > toDate!)
    {
      invoke.reject("Photo date filters are invalid")
      return
    }
    guard requirePhotosPermission(invoke) else { return }
    let limit = min(max(parameters?.limit ?? 50, 1), 200)
    let query = parameters?.query?.trimmingCharacters(in: .whitespacesAndNewlines).lowercased() ?? ""
    let options = PHFetchOptions()
    options.sortDescriptors = [NSSortDescriptor(key: "creationDate", ascending: false)]
    options.fetchLimit = limit
    let result: PHFetchResult<PHAsset>
    if let albumId = parameters?.albumId, !albumId.isEmpty {
      guard let collection = photoCollection(albumId) else {
        invoke.reject("Photo album ID is invalid or unavailable")
        return
      }
      result = PHAsset.fetchAssets(in: collection, options: options)
    } else {
      result = PHAsset.fetchAssets(with: options)
    }
    var photos = [PhotoAssetResult]()
    result.enumerateObjects { asset, _, stop in
      let item = self.photoAsset(asset)
      if let mediaType = parameters?.mediaType, mediaType != item.mediaType {
        return
      }
      if !query.isEmpty && !item.filename.lowercased().contains(query) {
        return
      }
      if let fromDate, asset.creationDate.map({ $0 < fromDate }) ?? false {
        return
      }
      if let toDate, asset.creationDate.map({ $0 > toDate }) ?? false {
        return
      }
      photos.append(item)
      if photos.count >= limit { stop.pointee = true }
    }
    invoke.resolve(
      PhotoListResult(
        photos: photos,
        count: photos.count,
        limited: authorizationState("photos") == "limited"
      )
    )
  }

  private func createPhotoAlbum(_ invoke: Invoke, parameters: OperationParameters?) {
    guard requirePhotosPermission(invoke) else { return }
    let name = parameters?.albumName?.trimmingCharacters(in: .whitespacesAndNewlines) ?? ""
    guard !name.isEmpty, name.count <= 120, !name.contains("/"), !name.unicodeScalars.contains(where: { CharacterSet.controlCharacters.contains($0) }) else {
      invoke.reject("Photo album name is invalid")
      return
    }
    var placeholder: PHObjectPlaceholder?
    PHPhotoLibrary.shared().performChanges({
      let request = PHAssetCollectionChangeRequest.creationRequestForAssetCollection(withTitle: name)
      placeholder = request.placeholderForCreatedAssetCollection
    }) { success, error in
      if !success {
        invoke.reject(error?.localizedDescription ?? "Unable to create photo album")
        return
      }
      invoke.resolve([
        "albumId": placeholder?.localIdentifier ?? "",
        "albumName": name,
        "created": true,
      ])
    }
  }

  private func addPhotosToAlbum(_ invoke: Invoke, parameters: OperationParameters?) {
    guard requirePhotosPermission(invoke) else { return }
    guard parameters?.confirmed == true else {
      invoke.reject("Adding photos to an album requires explicit confirmation")
      return
    }
    guard let ids = validPhotoIds(parameters?.assetIds),
      let albumId = parameters?.albumId,
      let collection = photoCollection(albumId)
    else {
      invoke.reject("Photo IDs or album ID are invalid")
      return
    }
    let assets = photoAssets(ids)
    guard assets.count == ids.count else {
      invoke.reject("One or more photos are unavailable under the current permission")
      return
    }
    PHPhotoLibrary.shared().performChanges({
      PHAssetCollectionChangeRequest(for: collection)?.addAssets(assets as NSArray)
    }) { success, error in
      if success {
        invoke.resolve(["albumId": albumId, "requested": ids.count, "added": assets.count])
      } else {
        invoke.reject(error?.localizedDescription ?? "Unable to add photos to album")
      }
    }
  }

  private func deletePhotos(_ invoke: Invoke, parameters: OperationParameters?) {
    guard requirePhotosPermission(invoke) else { return }
    guard parameters?.confirmed == true, let ids = validPhotoIds(parameters?.assetIds) else {
      invoke.reject("Deleting photos requires explicit confirmation and valid photo IDs")
      return
    }
    let assets = photoAssets(ids)
    guard assets.count == ids.count else {
      invoke.reject("One or more photos are unavailable under the current permission")
      return
    }
    PHPhotoLibrary.shared().performChanges({
      PHAssetChangeRequest.deleteAssets(assets as NSArray)
    }) { success, error in
      if success {
        invoke.resolve(["requested": ids.count, "deleted": assets.count])
      } else {
        invoke.reject(error?.localizedDescription ?? "Unable to delete photos")
      }
    }
  }

  private func openFile(_ invoke: Invoke, parameters: OperationParameters?) {
    let fileName = parameters?.fileName?.trimmingCharacters(in: .whitespacesAndNewlines) ?? ""
    let mimeType = parameters?.mimeType?.trimmingCharacters(in: .whitespacesAndNewlines) ?? ""
    let encoded = parameters?.data ?? ""
    guard
      !fileName.isEmpty,
      fileName.count <= 180,
      !fileName.contains("/"),
      !fileName.contains("\\"),
      !mimeType.isEmpty,
      mimeType.count <= 120,
      encoded.count <= 180 * 1024 * 1024,
      let data = Data(base64Encoded: encoded),
      !data.isEmpty,
      data.count <= 128 * 1024 * 1024
    else {
      invoke.reject("Asset file parameters are invalid")
      return
    }
    do {
      let directory = FileManager.default.temporaryDirectory
        .appendingPathComponent("PisperOpenAssets", isDirectory: true)
        .appendingPathComponent(UUID().uuidString, isDirectory: true)
      try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
      let fileURL = directory.appendingPathComponent(fileName, isDirectory: false)
      try data.write(to: fileURL, options: .atomic)
      DispatchQueue.main.async {
        guard let controller = self.manager.viewController else {
          invoke.reject("Unable to present the application picker")
          return
        }
        let sheet = UIActivityViewController(activityItems: [fileURL], applicationActivities: nil)
        if let popover = sheet.popoverPresentationController {
          popover.sourceView = controller.view
          popover.sourceRect = CGRect(
            x: controller.view.bounds.midX,
            y: controller.view.bounds.midY,
            width: 0,
            height: 0
          )
        }
        sheet.completionWithItemsHandler = { _, completed, _, error in
          if let error {
            invoke.reject(error.localizedDescription)
          } else {
            invoke.resolve(["opened": completed, "operation": "files.open"])
          }
        }
        controller.present(sheet, animated: true)
      }
    } catch {
      invoke.reject(error.localizedDescription)
    }
  }

  private func shareText(_ invoke: Invoke, parameters: OperationParameters?) {
    let text = parameters?.text?.trimmingCharacters(in: .whitespacesAndNewlines) ?? ""
    guard !text.isEmpty, text.count <= 100_000 else {
      invoke.reject("Share text is empty or too long")
      return
    }
    let title = parameters?.title?.trimmingCharacters(in: .whitespacesAndNewlines)
    DispatchQueue.main.async {
      guard let controller = self.manager.viewController else {
        invoke.reject("Unable to present the share sheet")
        return
      }
      let sheet = UIActivityViewController(activityItems: [text], applicationActivities: nil)
      if let title, !title.isEmpty { sheet.title = title }
      if let popover = sheet.popoverPresentationController {
        popover.sourceView = controller.view
        popover.sourceRect = CGRect(
          x: controller.view.bounds.midX,
          y: controller.view.bounds.midY,
          width: 0,
          height: 0
        )
      }
      sheet.completionWithItemsHandler = { _, completed, _, error in
        if let error {
          invoke.reject(error.localizedDescription)
        } else {
          invoke.resolve(["shared": completed])
        }
      }
      controller.present(sheet, animated: true)
    }
  }

  private func searchContacts(_ invoke: Invoke, parameters: OperationParameters?) {
    guard requirePermission("contacts", invoke: invoke) else { return }
    let query = parameters?.query?.trimmingCharacters(in: .whitespacesAndNewlines) ?? ""
    let limit = min(max(parameters?.limit ?? 50, 1), 200)
    DispatchQueue.global(qos: .userInitiated).async {
      do {
        let keys: [CNKeyDescriptor] = [
          CNContactIdentifierKey as CNKeyDescriptor,
          CNContactGivenNameKey as CNKeyDescriptor,
          CNContactFamilyNameKey as CNKeyDescriptor,
          CNContactOrganizationNameKey as CNKeyDescriptor,
          CNContactPhoneNumbersKey as CNKeyDescriptor,
        ]
        let request = CNContactFetchRequest(keysToFetch: keys)
        request.sortOrder = .userDefault
        var contacts = [ContactItem]()
        try self.contactStore.enumerateContacts(with: request) { contact, stop in
          let name = CNContactFormatter.string(from: contact, style: .fullName)
            ?? contact.organizationName
          let phones = contact.phoneNumbers.map { $0.value.stringValue }
          let searchable = ([name] + phones).joined(separator: " ")
          if query.isEmpty || searchable.localizedCaseInsensitiveContains(query) {
            contacts.append(ContactItem(id: contact.identifier, name: name, phones: phones))
          }
          if contacts.count >= limit { stop.pointee = true }
        }
        invoke.resolve(
          ContactResult(contacts: contacts, count: contacts.count, limited: contacts.count >= limit)
        )
      } catch {
        invoke.reject(error.localizedDescription)
      }
    }
  }

  private func capturePhoto(_ invoke: Invoke, parameters: OperationParameters?) {
    guard requirePermission("camera", invoke: invoke) else { return }
    guard UIImagePickerController.isSourceTypeAvailable(.camera) else {
      invoke.reject("Camera capture is unavailable on this device")
      return
    }
    cameraInvoke = invoke
    DispatchQueue.main.async {
      let picker = UIImagePickerController()
      picker.sourceType = .camera
      picker.cameraCaptureMode = .photo
      picker.cameraDevice = parameters?.cameraDirection == "front" ? .front : .rear
      picker.delegate = self
      guard let controller = self.manager.viewController else {
        self.cameraInvoke = nil
        invoke.reject("Unable to present the camera")
        return
      }
      controller.present(picker, animated: true)
    }
  }

  func imagePickerController(
    _ picker: UIImagePickerController,
    didFinishPickingMediaWithInfo info: [UIImagePickerController.InfoKey: Any]
  ) {
    guard let invoke = cameraInvoke, let image = info[.originalImage] as? UIImage else {
      picker.dismiss(animated: true)
      cameraInvoke?.reject("Camera did not return an image")
      cameraInvoke = nil
      return
    }
    let largest = max(image.size.width, image.size.height)
    let scale = largest > 1600 ? 1600 / largest : 1
    let size = CGSize(width: image.size.width * scale, height: image.size.height * scale)
    let renderer = UIGraphicsImageRenderer(size: size)
    let output = renderer.image { _ in image.draw(in: CGRect(origin: .zero, size: size)) }
    guard let data = output.jpegData(compressionQuality: 0.82) else {
      picker.dismiss(animated: true)
      invoke.reject("Unable to encode captured image")
      cameraInvoke = nil
      return
    }
    picker.dismiss(animated: true)
    invoke.resolve(
      PhotoResult(
        data: data.base64EncodedString(),
        mimeType: "image/jpeg",
        width: Int(size.width.rounded()),
        height: Int(size.height.rounded())
      )
    )
    cameraInvoke = nil
  }

  func imagePickerControllerDidCancel(_ picker: UIImagePickerController) {
    picker.dismiss(animated: true)
    cameraInvoke?.reject("Camera capture was cancelled")
    cameraInvoke = nil
  }

  private func currentLocation(_ invoke: Invoke) {
    guard requirePermission("location", invoke: invoke) else { return }
    guard CLLocationManager.locationServicesEnabled() else {
      invoke.reject("Location services are disabled")
      return
    }
    guard locationInvoke == nil else {
      invoke.reject("A location request is already active")
      return
    }
    locationInvoke = invoke
    DispatchQueue.main.async {
      self.locationManager.desiredAccuracy = kCLLocationAccuracyHundredMeters
      self.locationManager.requestLocation()
    }
  }

  func locationManagerDidChangeAuthorization(_ manager: CLLocationManager) {
    if let invoke = permissionInvoke, authorizationState("location") != "prompt" {
      permissionInvoke = nil
      invoke.resolve([
        "capability": "location",
        "state": authorizationState("location"),
      ])
    }
  }

  func locationManager(_ manager: CLLocationManager, didUpdateLocations locations: [CLLocation]) {
    guard let invoke = locationInvoke, let location = locations.last else { return }
    locationInvoke = nil
    invoke.resolve(
      LocationResult(
        latitude: location.coordinate.latitude,
        longitude: location.coordinate.longitude,
        accuracyMeters: location.horizontalAccuracy,
        altitudeMeters: location.verticalAccuracy >= 0 ? location.altitude : nil,
        timestamp: Int(location.timestamp.timeIntervalSince1970 * 1000)
      )
    )
  }

  func locationManager(_ manager: CLLocationManager, didFailWithError error: Error) {
    locationInvoke?.reject(error.localizedDescription)
    locationInvoke = nil
  }
}

@_cdecl("init_plugin_mobile_device")
func initPlugin() -> Plugin {
  return MobileDevicePlugin()
}
