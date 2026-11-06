#!/bin/sh
LWK_BINS="/opt/lwk-bins/bin"
export ELECTRS_LIQUID_EXEC="${LWK_BINS}/electrs_linux_esplora_027e38d3ebc2f85b28ae76f8f3448438ee4fc7b1_liquid"
export ELEMENTSD_EXEC="${LWK_BINS}/elements-23.3.1/bin/elementsd"
export BITCOIND_EXEC="${LWK_BINS}/bitcoin-26.0/bin/bitcoind"
export WATERFALLS_EXEC="${LWK_BINS}/waterfalls_c09b914"
export ASSET_REGISTRY_EXEC="${LWK_BINS}/asset_registry_server_5ecf533"
export JADE_EMULATOR_IMAGE_NAME=xenoky/local-jade-emulator
export JADE_EMULATOR_IMAGE_VERSION="1.0.27"
export PIN_SERVER_IMAGE_NAME=tulipan81/blind_pin_server
export PIN_SERVER_IMAGE_VERSION=v0.0.7
export ANDROID_NDK_HOME="${LWK_BINS}/android-ndk"
export LIB_EXT=$([ "$(uname)" = "Darwin" ] && echo "dylib" || echo "so")
export CLASSPATH="$CLASSPATH:${LWK_BINS}/jna-5.13.0.jar"
export PATH="$PATH:${LWK_BINS}/kotlinc/bin:${LWK_BINS}/openlogic-openjdk-11.0.21+9-linux-x64/bin:${LWK_BINS}/swift-5.5-RELEASE-ubuntu18.04/usr/bin"
export WASM_BINDGEN_TEST_TIMEOUT=60
export AMP2_MOCK_EXEC="$PWD/target/debug/amp2_mock"
