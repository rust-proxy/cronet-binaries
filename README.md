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

The build is orchestrated by the small Go CLI in [`cmd/build-naive`](cmd/build-naive)
(the only Go code in the repo — it just drives `get-clang.sh` → `gn gen` →
`ninja`). It depends only on `github.com/spf13/cobra`.

Subcommands:

| Command             | Purpose                                                        |
|---------------------|---------------------------------------------------------------|
| `build`             | `gn gen` + `ninja` → produce `libcronet.{a,so,dll}`           |
| `package`           | Copy libs to `lib/` and headers to `include/`, dump link flags |
| `download-toolchain`| Download clang + sysroot without building                      |
| `env`               | Print `CC`/`CXX`/`CGO_LDFLAGS` for cross-compiling consumers    |

## Build instructions

```bash
git clone --recursive --depth=1 <this-repo>
cd cronet-libs

# Linux (host target). Add --target=os/arch to cross-compile,
# or --libc=musl for static musl builds.
go run ./cmd/build-naive build
go run ./cmd/build-naive package
# or simply: make
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
go run ./cmd/build-naive --target=linux/arm64 download-toolchain
go run ./cmd/build-naive --target=linux/arm64 build
go run ./cmd/build-naive --target=linux/arm64 package

# musl (static):
go run ./cmd/build-naive --target=linux/amd64 --libc=musl build
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
