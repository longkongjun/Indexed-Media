// swift-tools-version: 6.0
import PackageDescription

let package = Package(
    name: "IndexedDesign",
    platforms: [.iOS(.v16), .tvOS(.v16)],
    products: [.library(name: "IndexedDesign", targets: ["IndexedDesign"])],
    targets: [.target(name: "IndexedDesign")]
)
