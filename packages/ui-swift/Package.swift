// swift-tools-version: 6.0
import PackageDescription

let package = Package(
    name: "MediaFlowUI",
    platforms: [.iOS(.v16)],
    products: [.library(name: "MediaFlowUI", targets: ["MediaFlowUI"])],
    targets: [.target(name: "MediaFlowUI")]
)
