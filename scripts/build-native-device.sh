#!/bin/sh
# Build the arm64 Apple package from a frozen checkout. Requires Rust Apple
# targets, Xcode tools and Swift 6; does not install tools or pair a device.
set -eu
repo=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
cd "$repo"
destination=${1:-target/native-device-package}
mkdir -p "$destination"
destination=$(CDPATH= cd -- "$destination" && pwd)
if [ -e "$destination/NessaDeviceClient.xcframework" ]; then
  echo "Choose a fresh destination; an XCFramework already exists there." >&2
  exit 1
fi
build=${CARGO_TARGET_DIR:-"$repo/target"}
mkdir -p "$build"
build=$(CDPATH= cd -- "$build" && pwd)
export CARGO_TARGET_DIR="$build"
export IPHONEOS_DEPLOYMENT_TARGET=18.0
export MACOSX_DEPLOYMENT_TARGET=15.0

cargo build --locked --release -p nessa-device-client \
  --target aarch64-apple-ios --target aarch64-apple-ios-sim
cargo build --locked --release -p nessa-device-client --lib
cargo build --locked --release -p nessa-device-client \
  --features bindgen --bin nessa-device-bindgen
mkdir -p "$destination/Sources" "$destination/Headers"
"$build/release/nessa-device-bindgen" generate \
  --library "$build/release/libnessa_device_client.dylib" --language swift \
  --out-dir "$destination/Sources"
mv "$destination/Sources/nessa_device_clientFFI.h" "$destination/Headers/"
mv "$destination/Sources/nessa_device_clientFFI.modulemap" "$destination/Headers/module.modulemap"
cp crates/nessa-device-client/swift/*.swift "$destination/Sources/"
xcodebuild -create-xcframework \
  -library "$build/aarch64-apple-ios/release/libnessa_device_client.a" -headers "$destination/Headers" \
  -library "$build/aarch64-apple-ios-sim/release/libnessa_device_client.a" -headers "$destination/Headers" \
  -library "$build/release/libnessa_device_client.a" -headers "$destination/Headers" \
  -output "$destination/NessaDeviceClient.xcframework"
cat > "$destination/Package.swift" <<'SWIFT'
// swift-tools-version: 6.0
import PackageDescription
let package = Package(
    name: "NessaNative",
    platforms: [.iOS(.v18), .macOS(.v15)],
    products: [.library(name: "NessaNative", targets: ["NessaNative"])],
    targets: [
        .binaryTarget(name: "nessa_device_clientFFI", path: "NessaDeviceClient.xcframework"),
        .target(name: "NessaNative", dependencies: ["nessa_device_clientFFI"], path: "Sources",
                linkerSettings: [.linkedLibrary("iconv")])
    ]
)
SWIFT
echo "Native Apple package: $destination"
