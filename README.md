# cronet-libs

Prebuilt **cronet** static / shared libraries from [naiveproxy](https://github.com/klzgrad/naiveproxy)
(Chromium's network stack: HTTP/2, HTTP/3 QUIC, ECH, …).

This repository contains **only the C library build pipeline** — there are no Go
bindings here. It produces:

- `libcronet.a`  — static library (Linux glibc/musl, macOS, iOS, tvOS, Android)
- `libcronet.so` — shared library (Linux glibc)
- `libcronet.dll`— shared library (Windows)

together with the C headers (`include/`) and the system link flags required to
link the static library (`lib/<target>/link_flags.txt`).

## Supported Platforms

| Target        | OS      | CPU     |
|---------------|---------|---------|
| android/386   | android | x86     |
| android/amd64 | android | x64     |
| android/arm   | android | arm     |
| android/arm64 | android | arm64   |
| darwin/amd64  | mac     | x64     |
| darwin/arm64  | mac     | arm64   |
| ios/arm64     | ios     | arm64   |
| ios/amd64     | ios     | amd64   |
| linux/386     | linux   | x86     |
| linux/amd64   | linux   | x64     |
| linux/arm     | linux   | arm     |
| linux/arm64   | linux   | arm64   |
| linux/loong64 | linux   | loong64 |
| windows/amd64 | win     | x64     |
| windows/arm64 | win     | arm64   |

## How it works

The build is orchestrated by [`just`](https://github.com/casey/just), whose
recipes run the single-file cargo script
[`scripts/build-naive.rs`](scripts/build-naive.rs) (`get-clang.sh` → `gn gen` →
`ninja`). The script depends only on the Rust standard library, so it compiles
with no network access.

**Requirements:** `just`, a Rust toolchain with `cargo -Zscript` support
(nightly), and the usual Chromium build deps (`gn`/`ninja`, a C/C++ toolchain).
No Go.

`just` recipes:

| Recipe                          | Purpose                                                        |
|---------------------------------|----------------------------------------------------------------|
| `just build [target] [libc]`    | `compile` + `package` in one go                                |
| `just compile [target] [libc]`  | `gn gen` + `ninja` → produce `libcronet.{a,so,dll}`            |
| `just package [target] [libc]`  | Copy libs to `lib/` and headers to `include/`, dump link flags |
| `just apple`                    | Build every Apple platform                                     |
| `just download-toolchain …`     | Download clang + sysroot without building                      |
| `just env [target] [libc]`      | Print `CC`/`CXX`/`CGO_LDFLAGS` for cross-compiling consumers    |

The script can also be invoked directly without `just`:
`cargo +nightly -Zscript scripts/build-naive.rs -- build --target linux/amd64`.

## Build instructions

```bash
git clone --recursive --depth=1 <this-repo>
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

On Windows / for dynamic linking, ship `libcronet.dll` / `libcronet.so` next to
your executable.
