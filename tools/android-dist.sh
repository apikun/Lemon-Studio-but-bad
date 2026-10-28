#!/bin/sh
# Copyright (C) 2026 ItsAlphaNeon
# SPDX-License-Identifier: GPL-3.0-or-later
# This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

# Build the Mango Studio Android APK (arm64). The gradle project's shape and this flow follow
# daijro's honeydew (GPL-3.0-or-later), https://github.com/daijro/honeydew.
# usage: tools/android-dist.sh [--debug]
set -eu
cd "$(dirname "$0")/.."

: "${ANDROID_HOME:?set ANDROID_HOME to the Android SDK directory}"
export ANDROID_SDK_ROOT=$ANDROID_HOME

# The NDK: the first one installed (CI images ship one; the setup puts r28b in place).
NDK_HOME=
for d in "$ANDROID_HOME"/ndk/*/ "$ANDROID_HOME"/android-ndk-*/; do
    [ -d "$d" ] && NDK_HOME="${d%/}" && break
done
[ -n "$NDK_HOME" ] || { echo "no NDK under $ANDROID_HOME (sdkmanager 'ndk;28.1.13356709')" >&2; exit 1; }
export ANDROID_NDK_HOME=$NDK_HOME
TC=$NDK_HOME/toolchains/llvm/prebuilt/linux-x86_64
export CARGO_TARGET_AARCH64_LINUX_ANDROID_LINKER=$TC/bin/aarch64-linux-android26-clang
export CC_aarch64_linux_android=$TC/bin/aarch64-linux-android26-clang
export CXX_aarch64_linux_android=$TC/bin/aarch64-linux-android26-clang++
export AR_aarch64_linux_android=$TC/bin/llvm-ar
export CARGO_BUILD_JOBS=${CARGO_BUILD_JOBS:-4}
# The desktop's include paths poison the NDK clang (host glibc headers against Android's libc).
unset CPATH BINDGEN_EXTRA_CLANG_ARGS C_INCLUDE_PATH CPLUS_INCLUDE_PATH 2>/dev/null || true

task=assembleRelease
case "${1:-}" in
    '') ;;
    --debug) task=assembleDebug ;;
    *) echo "usage: $0 [--debug]" >&2; exit 2 ;;
esac

build=${MANGO_ANDROID_BUILD_DIR:-target/android}
mkdir -p "$build/jniLibs/arm64-v8a"
cargo build --release --locked -p os-android --target aarch64-linux-android
cp target/aarch64-linux-android/release/libos_android.so "$build/jniLibs/arm64-v8a/"
android/gradlew -p android --no-daemon "$task"

apk=$(ls "$build"/outputs/apk/*/app-*.apk | head -1)
sdk_tools=$ANDROID_HOME/build-tools/36.0.0
"$sdk_tools/apksigner" verify "$apk"
"$sdk_tools/zipalign" -c -P 16 4 "$apk"
cp "$apk" Mango-Studio-android-arm64.apk
echo "wrote Mango-Studio-android-arm64.apk"
