#!/usr/bin/env bash
set -euo pipefail

# Build liblna_ffi.so for Android arm64-v8a and x86_64 using cargo-ndk.
# Prerequisites:
#   rustup target add aarch64-linux-android x86_64-linux-android
#   cargo install cargo-ndk
#   export ANDROID_NDK_HOME=/path/to/android-ndk

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../../.." && pwd)"
JNI_OUT="$SCRIPT_DIR/app/src/main/jniLibs"

mkdir -p "$JNI_OUT"
cd "$REPO_ROOT"

cargo ndk \
  -t arm64-v8a \
  -t x86_64 \
  -o "$JNI_OUT" \
  build --release -p lna-ffi

echo "Built Android native libraries into $JNI_OUT"
