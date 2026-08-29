import AVFoundation
import Contacts
import CoreLocation
import Tauri
import UIKit

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
  let packageName: String?
  let appUrl: String?
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

final class MobileDevicePlugin: Plugin, UIImagePickerControllerDelegate,
  UINavigationControllerDelegate, CLLocationManagerDelegate
{
  private let contactStore = CNContactStore()
  private let locationManager = CLLocationManager()
  private var permissionInvoke: Invoke?
  private var locationInvoke: Invoke?
  private var cameraInvoke: Invoke?

  override init() {
    super.init()
    locationManager.delegate = self
  }

  private func authorizationState(_ capability: String) -> String {
    switch capability {
    case "contacts":
      let status = CNContactStore.authorizationStatus(for: .contacts)
      if #available(iOS 18.0, *), status.rawValue == 4 { return "granted" }
      switch status {
      case .authorized: return "granted"
      case .denied, .restricted: return "denied"
      case .notDetermined: return "prompt"
      @unknown default: return "prompt"
      }
    case "camera":
      switch AVCaptureDevice.authorizationStatus(for: .video) {
      case .authorized: return "granted"
      case .denied, .restricted: return "denied"
      case .notDetermined: return "prompt"
      @unknown default: return "prompt"
      }
    case "location":
      switch locationManager.authorizationStatus {
      case .authorizedWhenInUse, .authorizedAlways: return "granted"
      case .denied, .restricted: return "denied"
      case .notDetermined: return "prompt"
      @unknown default: return "prompt"
      }
    case "externalApps": return "not-required"
    default:
      return "unsupported"
    }
  }

  @objc public func permissionStates(_ invoke: Invoke) {
    invoke.resolve([
      "contacts": authorizationState("contacts"),
      "camera": authorizationState("camera"),
      "location": authorizationState("location"),
      "externalApps": authorizationState("externalApps"),
    ])
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
    case "apps.open_url": openURL(invoke, parameters: args.parameters)
    case "apps.open_map": openMap(invoke, parameters: args.parameters)
    case "apps.open_system_settings": openSystemSettings(invoke)
    case "apps.open_dialer": openDialer(invoke, parameters: args.parameters)
    case "apps.compose_sms": composeSMS(invoke, parameters: args.parameters)
    case "apps.open_app": openApp(invoke, parameters: args.parameters)
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
