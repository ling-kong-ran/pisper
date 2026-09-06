// swift-tools-version:5.9
import PackageDescription

let package = Package(
  name: "pisper-mobile-device-plugin",
  platforms: [.iOS("15.1")],
  products: [
    .library(
      name: "pisper-mobile-device-plugin",
      type: .static,
      targets: ["pisper-mobile-device-plugin"])
  ],
  dependencies: [
    .package(name: "Tauri", path: "../.tauri/tauri-api"),
    .package(url: "https://github.com/k2-fsa/sherpa-onnx.git", exact: "1.13.7"),
    .package(url: "https://github.com/Lakr233/libarchive.xcframework.git", exact: "0.1.1")
  ],
  targets: [
    .target(
      name: "pisper-mobile-device-plugin",
      dependencies: [
        .byName(name: "Tauri"),
        .product(name: "sherpa-onnx", package: "sherpa-onnx"),
        .product(name: "LibArchive", package: "libarchive.xcframework")
      ],
      path: "Sources",
      resources: [.copy("SpeechResources")],
      linkerSettings: [
        .linkedFramework("AVFoundation"),
        .linkedFramework("CoreFoundation"),
        .linkedFramework("Foundation"),
        .linkedFramework("CoreML"),
        .linkedLibrary("c++")
      ]),
    .testTarget(
      name: "SpeechTests",
      dependencies: [.byName(name: "pisper-mobile-device-plugin")],
      path: "Tests")
  ]
)
