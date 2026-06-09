# Build orchestration for the cronet libraries.
#
# Recipes drive scripts/build-naive.sh (a bash port of the old Go tool).
# No Go toolchain is required — only bash, plus the usual Chromium build
# deps (gn/ninja, a C toolchain, etc.).
#
# Usage:
#   just                      # list recipes
#   just build                # host target (compile + package)
#   just build linux/arm64    # cross-compile
#   just build linux/amd64 musl
#   just apple                # all Apple platforms

set shell := ["bash", "-c"]

script := "scripts/build-naive.sh"

# List available recipes.
default:
    @just --list

# Compile + package cronet libs for TARGET (e.g. linux/amd64; empty = host).
# Optional LIBC=musl for static musl Linux builds.
build target="" libc="":
    bash {{script}} build   --target "{{target}}" --libc "{{libc}}"
    bash {{script}} package --target "{{target}}" --libc "{{libc}}"

# Only run gn gen + ninja (the expensive, cacheable step) for TARGET.
compile target="" libc="":
    bash {{script}} build --target "{{target}}" --libc "{{libc}}"

# Copy built libs/headers into lib/ + include/ and dump link flags for TARGET.
package target="" libc="":
    bash {{script}} package --target "{{target}}" --libc "{{libc}}"

# Build all Apple platforms (macOS, iOS, tvOS, and simulators).
apple:
    bash {{script}} build   --target "ios/arm64,ios/arm64/simulator,ios/amd64/simulator,tvos/arm64,tvos/arm64/simulator,tvos/amd64/simulator,darwin/arm64,darwin/amd64"
    bash {{script}} package --target "ios/arm64,ios/arm64/simulator,ios/amd64/simulator,tvos/arm64,tvos/arm64/simulator,tvos/amd64/simulator,darwin/arm64,darwin/amd64"

# Download clang + sysroot for TARGET without building.
download-toolchain target="" libc="":
    bash {{script}} download-toolchain --target "{{target}}" --libc "{{libc}}"

# Print CC/CXX/CGO_LDFLAGS env for cross-compiling consumers of TARGET.
env target="" libc="":
    bash {{script}} env --target "{{target}}" --libc "{{libc}}"

# Same as `env`, prefixed with `export ` for use with eval.
env-export target="" libc="":
    bash {{script}} env --target "{{target}}" --libc "{{libc}}" --export
