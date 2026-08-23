// swift-tools-version:5.9
import PackageDescription

let package = Package(
  name: "pisper-mobile-device-plugin",
  platforms: [.iOS(.v14)],
  products: [
    .library(
      name: "pisper-mobile-device-plugin",
      type: .static,
      targets: ["pisper-mobile-device-plugin"])
  ],
  dependencies: [
    .package(name: "Tauri", path: "../.tauri/tauri-api")
  ],
  targets: [
    .target(
      name: "pisper-mobile-device-plugin",
      dependencies: [.byName(name: "Tauri")],
      path: "Sources")
  ]
)
