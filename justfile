# Build orchestration for the cronet libraries.
#
# Recipes drive scripts/build-cronet.rs, a single-file cargo script (std only).
# Requires a Rust toolchain that supports `cargo -Zscript` (nightly), plus the
# usual Chromium build deps (gn/ninja, a C/C++ toolchain). No Go needed.
#
# Usage:
#   just                      # list recipes
#   just build                # host target (compile + package)
#   just build linux/arm64    # cross-compile
#   just build linux/amd64 musl
#   just apple                # all Apple platforms

# bash on every platform (Git bash on Windows). The cargo script spawns gn.exe
# natively via std::process::Command, so MSYS path-mangling never touches the
# gn arguments — only this thin `cargo` invocation runs under bash.
set shell := ["bash", "-c"]

script := "scripts/build-cronet.rs"
run := "cargo +nightly -Zscript " + script + " --"

# List available recipes.
default:
    @just --list

# Compile + package cronet libs for TARGET (e.g. linux/amd64; empty = host).
# Optional LIBC=musl for static musl Linux builds.
build target="" libc="":
    {{run}} build   --target "{{target}}" --libc "{{libc}}"
    {{run}} package --target "{{target}}" --libc "{{libc}}"

# Only run gn gen + ninja (the expensive, cacheable step) for TARGET.
compile target="" libc="":
    {{run}} build --target "{{target}}" --libc "{{libc}}"

# Copy built libs/headers into lib/ + include/ and dump link flags for TARGET.
package target="" libc="":
    {{run}} package --target "{{target}}" --libc "{{libc}}"

# Build all Apple platforms (macOS, iOS, tvOS, and simulators).
apple:
    {{run}} build   --target "ios/arm64,ios/arm64/simulator,ios/amd64/simulator,tvos/arm64,tvos/arm64/simulator,tvos/amd64/simulator,darwin/arm64,darwin/amd64"
    {{run}} package --target "ios/arm64,ios/arm64/simulator,ios/amd64/simulator,tvos/arm64,tvos/arm64/simulator,tvos/amd64/simulator,darwin/arm64,darwin/amd64"

# Download clang + sysroot for TARGET without building.
download-toolchain target="" libc="":
    {{run}} download-toolchain --target "{{target}}" --libc "{{libc}}"

# Print CC/CXX/CGO_LDFLAGS env for cross-compiling consumers of TARGET.
env target="" libc="":
    {{run}} env --target "{{target}}" --libc "{{libc}}"

# Same as `env`, prefixed with `export ` for use with eval.
env-export target="" libc="":
    {{run}} env --target "{{target}}" --libc "{{libc}}" --export
