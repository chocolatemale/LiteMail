#!/bin/bash
# Builds the Rust core as a static library, generates the Swift bindings with UniFFI and
# packages them as an xcframework that the Swift package links.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
export PATH="/opt/homebrew/opt/rustup/bin:$HOME/.cargo/bin:$PATH"
cd "$ROOT/core"

# The app targets macOS 26; build C dependencies (SQLite) for the same minimum.
export MACOSX_DEPLOYMENT_TARGET=26.0
cargo build --release --lib
GEN="$ROOT/build/gen"
rm -rf "$GEN" && mkdir -p "$GEN"
cargo run --release --quiet --bin uniffi-bindgen -- generate \
  --library target/release/liblitemail_core.dylib --language swift --out-dir "$GEN"

HDR="$ROOT/build/ffi-headers"
rm -rf "$HDR" && mkdir -p "$HDR"
cp "$GEN/litemail_coreFFI.h" "$HDR/"
cp "$GEN/litemail_coreFFI.modulemap" "$HDR/module.modulemap"

XCF="$ROOT/app/Frameworks/litemail_coreFFI.xcframework"
rm -rf "$XCF"
xcodebuild -create-xcframework -library target/release/liblitemail_core.a -headers "$HDR" -output "$XCF" >/dev/null

mkdir -p "$ROOT/app/Sources/LiteMailCore"
cp "$GEN/litemail_core.swift" "$ROOT/app/Sources/LiteMailCore/litemail_core.swift"
echo "core: $(du -h target/release/liblitemail_core.a | cut -f1) static lib, bindings regenerated"
