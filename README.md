# cronet-libs

Prebuilt **cronet** static / shared libraries from [naiveproxy](https://github.com/klzgrad/naiveproxy)
(Chromium's network stack: HTTP/2, HTTP/3 QUIC, ECH, …).

This repository contains **only the C library build pipeline** — there are no Go
bindings here. It produces:

- `libcronet.a`  — static library (Linux glibc/musl, macOS, iOS, tvOS, Android)
- `libcronet.so` — shared library (Linux glibc)
- `libcronet.dylib` — shared library (macOS, iOS)
- `cronet.lib`  — static library (Windows MSVC)
- `libcronet.dll`— shared library (Windows)

together with the C headers (`include/`) and the system link flags required to
link the static library (`lib/<target>/link_flags.txt`).

The Chromium 154 build uses SagerNet's `cronet-go-dev-v154` dialer ABI. Use matching 154 Rust bindings for these libraries.

Pushing a Git tag runs the build matrix and publishes the libraries and
`SHA256SUMS` in a release with the same tag. The `latest` tag is reserved
for the existing rolling release.

Pull requests and pushes to `main` run the same build job for the continuously
tested Apple targets (`ci = true` in [target.toml](.github/target.toml),
currently macOS arm64 and iOS arm64): both dylibs are built and checked for
their install name and exported symbols, and the macOS arm64 dylib is linked
and executed by [`tests/dylib-smoke.c`](tests/dylib-smoke.c). These runs never
publish a release; only tag pushes do.

## Supported Platforms

| Target        | OS      | CPU     | dynamic | static |
|---------------|---------|---------|:-------:|:------:|
| android/386   | android | x86     | —      | ✅     |
| android/amd64 | android | x64     | —      | ✅     |
| android/arm   | android | arm     | —      | ✅     |
| android/arm64 | android | arm64   | —      | ✅     |
| darwin/amd64  | mac     | x64     | ✅      | ✅     |
| darwin/arm64  | mac     | arm64   | ✅      | ✅     |
| ios/arm64     | ios     | arm64   | ✅      | ✅     |
| ios/amd64     | ios     | amd64   | ✅      | ✅     |
| linux/386     | linux   | x86     | ✅      | ✅     |
| linux/amd64   | linux   | x64     | ✅      | ✅     |
| linux/arm     | linux   | arm     | ✅      | ✅     |
| linux/arm64   | linux   | arm64   | ✅      | ✅     |
| linux/loong64 | linux   | loong64 | ✅      | ✅     |
| windows/amd64 | win     | x64     | ✅      | ✅     |
| windows/arm64 | win     | arm64   | ✅      | ✅     |

On Linux, shared libraries require glibc. Static libraries are available for both glibc and musl.
The macOS and iOS shared libraries are plain Mach-O dylibs; an iOS app must embed and
re-sign `libcronet.dylib` inside the app bundle. tvOS stays static-only.

## How it works

The build is orchestrated by [`just`](https://github.com/casey/just), whose
recipes run the single-file cargo script
[`scripts/build-cronet.rs`](scripts/build-cronet.rs) (`get-clang.sh` → `gn gen` →
`ninja`). The script depends only on the Rust standard library, so it compiles
with no network access.

**Requirements:** `just`, a Rust toolchain with `cargo -Zscript` support
(nightly), and the usual Chromium build deps (`gn`/`ninja`, a C/C++ toolchain).
No Go.

`just` recipes:

| Recipe                          | Purpose                                                        |
|---------------------------------|----------------------------------------------------------------|
| `just build [target] [libc]`    | `compile` + `package` in one go                                |
| `just compile [target] [libc]`  | `gn gen` + `ninja` → produce `libcronet.{a,so,dylib,dll}`       |
| `just package [target] [libc]`  | Copy libs to `lib/` and headers to `include/`, dump link flags |
| `just apple`                    | Build every Apple platform                                     |
| `just download-toolchain …`     | Download clang + sysroot without building                      |
| `just env [target] [libc]`      | Print `CC`/`CXX`/`CGO_LDFLAGS` for cross-compiling consumers    |

The script can also be invoked directly without `just`:
`cargo +nightly -Zscript scripts/build-cronet.rs -- build --target linux/amd64`.

## Build instructions

```bash
# --shallow-submodules keeps the naiveproxy submodule a depth-1 clone
# (otherwise it pulls ~124 MB of history).
git clone --recursive --shallow-submodules --depth=1 <this-repo>
cd cronet-libs

just                       # list recipes
just build                 # host target (compile + package)
just build linux/arm64     # cross-compile
just build linux/amd64 musl # static musl Linux build
just apple                 # all Apple platforms
```

Outputs land in:

```
lib/<os>_<arch>/libcronet.a        # static lib
lib/<os>_<arch>/libcronet.so       # Linux glibc shared lib
lib/<os>_<arch>/libcronet.dylib    # macOS / iOS shared lib
lib/<os>_<arch>/libcronet.dll      # Windows shared lib
lib/<os>_<arch>/link_flags.txt     # system libs/frameworks needed to link .a
include/*.h                        # C headers
```

The raw ninja output also remains under
`naiveproxy/src/out/cronet-<os>-<cpu>/` if you prefer to grab it directly.

### Cross-compiling

```bash
just download-toolchain linux/arm64
just build linux/arm64

# musl (static):
just build linux/amd64 musl
```

### Directories worth caching (CI)

```yaml
- naiveproxy/src/third_party/llvm-build/
- naiveproxy/src/gn/out/
- naiveproxy/src/chrome/build/pgo_profiles/
- naiveproxy/src/out/sysroot-build/
```

## Linking against the static library

`libcronet.a` needs a number of system libraries; the exact set is written to
`lib/<target>/link_flags.txt` at package time. Example (Linux):

```bash
cc myapp.c -Iinclude lib/linux_amd64/libcronet.a $(cat lib/linux_amd64/link_flags.txt | grep -v '^#')
```

On Windows / for dynamic linking, ship `libcronet.dll` / `libcronet.so` /
`libcronet.dylib` next to your executable.

Linux release archives use `libcronet-linux-<arch>.a` (glibc) and
`libcronet-linux-<arch>_musl.a` (musl). Windows MSVC archives use
`libcronet-windows-<arch>.lib`; these are real static archives, not DLL import
libraries. Apple releases use `libcronet-darwin-<arch>.a` /
`libcronet-ios-<variant>.a` for static linking and the matching `.dylib` assets
for dynamic loading. Matching `.link-flags.txt` assets record native linker
dependencies. Existing `.so`, `.dylib` and `.dll` assets remain available for
dynamic consumers.

macOS and iOS shared libraries carry the install name
`@rpath/libcronet.dylib` and export only the public C API (`Cronet_*` and
`bidirectional_stream_*`); all other Chromium, libc++ and prefixed BoringSSL
symbols stay private to the dylib. Packaged dylibs are ad-hoc signed
(`codesign --force --sign -`) because Apple silicon refuses to map unsigned
code. On macOS the dylib can be loaded from a normal search path or with an
explicit path. iOS does not provide such search paths, so an app must embed
`libcronet.dylib` in its bundle (for example under `Frameworks/`) and let Xcode
re-sign it; load it by its absolute path inside the bundle.

Windows provides both CRT variants of the complete static Cronet archive:

- `libcronet-windows-<arch>_md.lib` uses `/MD`, matching Rust's default MSVC
  runtime. No `+crt-static` flag is needed. Build with `just build windows/amd64/md`
  (or `windows/arm64/md`). The companion metadata includes `msvcprt.lib`.
- `libcronet-windows-<arch>.lib` retains `/MT` for consumers explicitly using
  `-C target-feature=+crt-static`. Its metadata includes `libcpmt.lib`.

Both variants statically link Cronet; `/MD` depends on the MSVC runtime DLLs.
Use the matching `.link-flags.txt` file and the same CRT as the application.
The `/MD` build disables Chromium's static-CRT allocator shim so allocations
use the shared CRT. CI verifies both variants against the Rust bindings and
an independent BoringSSL consumer.

Cronet's private BoringSSL functions, data, opaque types and C++ namespace are
prefixed at source level so applications can also link another BoringSSL copy
(for example Quiche or `boring`). The build first discovers target-specific
symbols, including assembly, then rebuilds with generated prefix headers and
audits the completed archive. Linux x64 and Windows x64 CI additionally links
and initializes Cronet alongside the Rust `boring` crate.
