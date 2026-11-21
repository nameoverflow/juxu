#!/bin/sh
set -eu

export PATH="$HOME/.cargo/bin:/opt/homebrew/bin:/usr/local/bin:$PATH"

REPO_ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
OUT_DIR="$REPO_ROOT/iOS/build-lib"
mkdir -p "$OUT_DIR"

if [ "${PLATFORM_NAME:-iphoneos}" = "iphonesimulator" ]; then
    TARGET="aarch64-apple-ios-sim"
else
    TARGET="aarch64-apple-ios"
fi

cargo build --release --manifest-path "$REPO_ROOT/Core/Cargo.toml" --lib --target "$TARGET"
cp "$REPO_ROOT/Core/target/$TARGET/release/libjuxu_core.a" "$OUT_DIR/libjuxu_core.a"
