#!/usr/bin/env bash
#
# build-naive.sh — build orchestration for the cronet libraries.
#
# A faithful bash port of the former Go tool cmd/build-naive. It drives
# naiveproxy's get-clang.sh, then `gn gen` + `ninja`, and packages the
# resulting .a/.so/.dll into lib/ + include/.
#
# Subcommands:
#   build              gn gen + ninja  -> libcronet.{a,so,dll}
#   package            copy libs/headers into lib/ + include/, dump link flags
#   download-toolchain download clang + sysroot without building
#   env                print CC/CXX/CGO_LDFLAGS for cross-compiling consumers
#
# Flags: --target <list>   comma separated (e.g. linux/amd64,darwin/arm64);
#                          empty = host, "all" = every supported target
#        --libc <glibc|musl>   musl => static OpenWrt/musl Linux build
#        --export              (env only) prefix output with `export `
#
# Written for bash 3.2+ (macOS ships 3.2); avoid set -u (empty-array hazard).

set -eo pipefail

LIB_SELF="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(dirname "$LIB_SELF")"
NAIVE_ROOT="$PROJECT_ROOT/naiveproxy"
SRC_ROOT="$NAIVE_ROOT/src"

log()  { echo "[build] $*" >&2; }
die()  { echo "[build] ERROR: $*" >&2; exit 1; }

ALL_SPECS="linux/amd64 linux/arm64 linux/386 linux/arm linux/loong64 linux/mipsle linux/mips64le linux/riscv64 darwin/amd64 darwin/arm64 windows/amd64 windows/arm64 ios/arm64 ios/arm64/simulator ios/amd64 tvos/arm64 tvos/arm64/simulator tvos/amd64 android/arm64 android/amd64 android/arm android/386"

# ---------------------------------------------------------------------------
# Host detection
# ---------------------------------------------------------------------------

host_goos() {
  case "$(uname -s)" in
    Linux) echo linux ;;
    Darwin) echo darwin ;;
    MINGW*|MSYS*|CYGWIN*) echo windows ;;
    *) echo unknown ;;
  esac
}

host_goarch() {
  case "$(uname -m)" in
    x86_64|amd64) echo amd64 ;;
    aarch64|arm64) echo arm64 ;;
    i686|i386) echo 386 ;;
    armv7l|armv6l|arm) echo arm ;;
    loongarch64) echo loong64 ;;
    mips64el) echo mips64le ;;
    mipsel) echo mipsle ;;
    riscv64) echo riscv64 ;;
    *) echo unknown ;;
  esac
}

arch_to_cpu() {
  case "$1" in
    amd64) echo x64 ;;
    arm64) echo arm64 ;;
    386) echo x86 ;;
    arm) echo arm ;;
    loong64) echo loong64 ;;
    mipsle) echo mipsel ;;
    mips64le) echo mips64el ;;
    *) echo "$1" ;;
  esac
}

# ---------------------------------------------------------------------------
# Target resolution -> sets T_OS T_CPU T_GOOS T_ARCH T_LIBC T_PLATFORM T_ENV
# ---------------------------------------------------------------------------

resolve_target() {
  local spec="$1" libc="$2"
  T_OS=""; T_CPU=""; T_GOOS=""; T_ARCH=""; T_LIBC=""; T_PLATFORM=""; T_ENV=""

  if [ -z "$spec" ]; then
    spec="$(host_goos)/$(host_goarch)"
  fi

  local goos goarch variant
  goos="${spec%%/*}"
  local rest="${spec#*/}"
  goarch="${rest%%/*}"
  if [ "$rest" != "$goarch" ]; then
    variant="${rest#*/}"
  else
    variant=""
  fi

  case "$goos" in
    ios|tvos)
      resolve_apple "$goos" "$goarch" "$variant"
      return
      ;;
    linux)  T_OS=linux ;;
    darwin) T_OS=mac ;;
    windows) T_OS=win ;;
    android) T_OS=android ;;
    *) die "unsupported target: $spec" ;;
  esac

  T_GOOS="$goos"
  T_ARCH="$goarch"
  T_CPU="$(arch_to_cpu "$goarch")"

  if [ -n "$libc" ] && [ "$libc" != glibc ]; then
    [ "$libc" = musl ] || die "invalid libc: $libc (expected glibc or musl)"
    [ "$goos" = linux ] || die "--libc=musl is only supported for Linux targets, not $goos"
    T_OS=openwrt
    T_LIBC=musl
  fi
}

resolve_apple() {
  local goos="$1" goarch="$2" variant="$3"
  T_GOOS=ios
  T_OS=ios
  T_ARCH="$goarch"
  T_CPU="$(arch_to_cpu "$goarch")"
  T_LIBC=""
  if [ "$goos" = tvos ]; then T_PLATFORM=tvos; else T_PLATFORM=iphoneos; fi
  if [ "$variant" = simulator ]; then
    T_ENV=simulator
  elif [ "$goarch" = amd64 ]; then
    T_ENV=simulator
  else
    T_ENV=device
  fi
}

format_target() {
  if [ -n "$T_PLATFORM" ]; then
    local pn=iOS
    [ "$T_PLATFORM" = tvos ] && pn=tvOS
    if [ "$T_ENV" = simulator ]; then echo "$pn Simulator $T_ARCH"; else echo "$pn $T_ARCH"; fi
  elif [ "$T_LIBC" = musl ]; then
    echo "$T_GOOS/$T_ARCH (musl)"
  else
    echo "$T_GOOS/$T_ARCH"
  fi
}

output_dir() {
  if [ -n "$T_PLATFORM" ]; then
    local d="out/cronet-$T_PLATFORM-$T_CPU"
    [ "$T_ENV" = simulator ] && d="$d-simulator"
    echo "$d"
  else
    echo "out/cronet-$T_OS-$T_CPU"
  fi
}

lib_dir_name() {
  local osn="$T_GOOS"
  [ "$T_PLATFORM" = tvos ] && osn=tvos
  local n="${osn}_${T_ARCH}"
  [ "$T_ENV" = simulator ] && n="${n}_simulator"
  [ "$T_LIBC" = musl ] && n="${n}_musl"
  echo "$n"
}

# ---------------------------------------------------------------------------
# OpenWrt / musl config -> sets OW_TARGET OW_SUBTARGET OW_ARCH OW_RELEASE
#                          OW_GCC OW_EXTRA[]
# ---------------------------------------------------------------------------

openwrt_config() {
  OW_EXTRA=()
  case "$T_CPU" in
    x64)     OW_TARGET=x86;          OW_SUBTARGET=64;      OW_ARCH=x86_64;       OW_RELEASE=23.05.5; OW_GCC=12.3.0 ;;
    arm64)   OW_TARGET=armsr;        OW_SUBTARGET=armv8;   OW_ARCH=aarch64;      OW_RELEASE=23.05.5; OW_GCC=12.3.0 ;;
    x86)     OW_TARGET=x86;          OW_SUBTARGET=generic; OW_ARCH=i386_pentium4; OW_RELEASE=23.05.5; OW_GCC=12.3.0 ;;
    arm)     OW_TARGET=armsr;        OW_SUBTARGET=armv7;   OW_ARCH="arm_cortex-a15_neon-vfpv4"; OW_RELEASE=23.05.5; OW_GCC=12.3.0 ;;
    loong64) OW_TARGET=loongarch64;  OW_SUBTARGET=generic; OW_ARCH=loongarch64;  OW_RELEASE=24.10.5; OW_GCC=13.3.0 ;;
    mipsel)  OW_TARGET=ramips;       OW_SUBTARGET=rt305x;  OW_ARCH=mipsel_24kc;  OW_RELEASE=23.05.5; OW_GCC=12.3.0; OW_EXTRA=('mips_float_abi="soft"' 'mips_arch_variant="r2"') ;;
    riscv64) OW_TARGET=sifiveu;      OW_SUBTARGET=generic; OW_ARCH=riscv64;      OW_RELEASE=23.05.5; OW_GCC=12.3.0 ;;
    *) die "unsupported CPU for musl: $T_CPU" ;;
  esac
}

# Debian sysroot path for a glibc CPU (ignores libc).
glibc_sysroot_path() {
  local cpu="$1" arch rel
  case "$cpu" in
    x64)      arch=amd64;    rel=bullseye ;;
    arm64)    arch=arm64;    rel=bullseye ;;
    x86)      arch=i386;     rel=bullseye ;;
    arm)      arch=armhf;    rel=bullseye ;;
    loong64)  arch=loong64;  rel=sid ;;
    mipsel)   arch=mipsel;   rel=bullseye ;;
    mips64el) arch=mips64el; rel=bullseye ;;
    riscv64)  arch=riscv64;  rel=trixie ;;
    *) die "no sysroot for cpu: $cpu" ;;
  esac
  echo "$SRC_ROOT/out/sysroot-build/$rel/${rel}_${arch}_staging"
}

sysroot_path() {
  if [ "$T_LIBC" = musl ]; then
    openwrt_config
    echo "$SRC_ROOT/out/sysroot-build/openwrt/$OW_RELEASE/$OW_ARCH"
  else
    glibc_sysroot_path "$T_CPU"
  fi
}

clang_target() {
  if [ "$T_LIBC" = musl ]; then
    case "$T_CPU" in
      x64)     echo x86_64-openwrt-linux-musl ;;
      arm64)   echo aarch64-openwrt-linux-musl ;;
      x86)     echo i486-openwrt-linux-musl ;;
      arm)     echo arm-openwrt-linux-musleabi ;;
      loong64) echo loongarch64-openwrt-linux-musl ;;
      mipsel)  echo mipsel-openwrt-linux-musl ;;
      riscv64) echo riscv64-openwrt-linux-musl ;;
    esac
    return
  fi
  case "$T_CPU" in
    x64)      echo x86_64-linux-gnu ;;
    arm64)    echo aarch64-linux-gnu ;;
    x86)      echo i686-linux-gnu ;;
    arm)      echo arm-linux-gnueabihf ;;
    loong64)  echo loongarch64-linux-gnu ;;
    mipsel)   echo mipsel-linux-gnu ;;
    mips64el) echo mips64el-linux-gnuabi64 ;;
    riscv64)  echo riscv64-linux-gnu ;;
  esac
}

# ---------------------------------------------------------------------------
# Toolchain
# ---------------------------------------------------------------------------

run_get_clang() {
  local hgoos; hgoos="$(host_goos)"

  if [ "$hgoos" = linux ] && { [ "$T_OS" = linux ] || [ "$T_OS" = android ] || [ "$T_OS" = openwrt ]; }; then
    local hcpu; hcpu="$(arch_to_cpu "$(host_goarch)")"
    local host_flags="target_os=\"linux\" target_cpu=\"$hcpu\""
    log "get-clang.sh (host sysroot) EXTRA_FLAGS=$host_flags"
    ( cd "$SRC_ROOT" && EXTRA_FLAGS="$host_flags" bash ./get-clang.sh )

    local host_src host_dst
    host_src="$(glibc_sysroot_path "$hcpu")"
    host_dst="$SRC_ROOT/build/linux/debian_bullseye_amd64-sysroot"
    if [ ! -e "$host_dst" ]; then
      log "symlink host sysroot $host_dst -> $host_src"
      ln -s "$host_src" "$host_dst"
    fi
  fi

  local extra="target_os=\"$T_OS\" target_cpu=\"$T_CPU\""
  if [ "$T_OS" = openwrt ]; then
    openwrt_config
    local ow="target=\"$OW_TARGET\" subtarget=\"$OW_SUBTARGET\" arch=\"$OW_ARCH\" release=\"$OW_RELEASE\" gcc_ver=\"$OW_GCC\""
    log "get-clang.sh EXTRA_FLAGS=$extra OPENWRT_FLAGS=$ow"
    ( cd "$SRC_ROOT" && EXTRA_FLAGS="$extra" OPENWRT_FLAGS="$ow" bash ./get-clang.sh )
  else
    log "get-clang.sh EXTRA_FLAGS=$extra"
    ( cd "$SRC_ROOT" && EXTRA_FLAGS="$extra" bash ./get-clang.sh )
  fi
}

# ---------------------------------------------------------------------------
# Build (gn gen + ninja)
# ---------------------------------------------------------------------------

compile_target() {
  run_get_clang

  local outdir; outdir="$(output_dir)"
  local hgoos; hgoos="$(host_goos)"

  local args
  args=(
    is_official_build=true
    is_debug=false
    is_clang=true
    use_clang_modules=false
    use_thin_lto=false
    fatal_linker_warnings=false
    treat_warnings_as_errors=false
    is_cronet_build=true
    use_udev=false
    use_aura=false
    use_ozone=false
    use_gio=false
    use_glib=false
    use_kerberos=false
    disable_zstd_filter=false
    enable_reporting=false
    enable_bracketed_proxy_uris=true
    enable_quic_proxy_support=true
    use_nss_certs=false
    enable_backup_ref_ptr_support=false
    enable_dangling_raw_ptr_checks=false
    exclude_unwind_tables=true
    enable_resource_allowlist_generation=false
    symbol_level=0
    enable_dsyms=false
    optimize_for_size=true
    "target_os=\"$T_OS\""
    "target_cpu=\"$T_CPU\""
  )

  case "$T_OS" in
    mac)
      args+=(use_sysroot=false)
      ;;
    linux)
      local sp rel
      sp="$(sysroot_path)"
      rel="${sp#"$SRC_ROOT"/}"
      args+=(use_sysroot=true "target_sysroot=\"//$rel\"")
      [ "$T_CPU" = x64 ] && args+=(use_cfi_icall=false is_cfi=false)
      ;;
    openwrt)
      openwrt_config
      local sd="out/sysroot-build/openwrt/$OW_RELEASE/$OW_ARCH"
      args+=(use_sysroot=true "target_sysroot=\"//$sd\"" build_static=true use_allocator_shim=false use_partition_alloc=false)
      [ "${#OW_EXTRA[@]}" -gt 0 ] && args+=("${OW_EXTRA[@]}")
      [ "$T_CPU" = x64 ] && args+=(use_cfi_icall=false is_cfi=false)
      ;;
    win)
      args+=(use_sysroot=false)
      ;;
    android)
      args+=(use_sysroot=false default_min_sdk_version=23)
      ;;
    ios)
      local plat="$T_PLATFORM" envv="$T_ENV"
      [ -z "$plat" ] && plat=iphoneos
      [ -z "$envv" ] && envv=device
      args+=(
        use_sysroot=false
        ios_enable_code_signing=false
        "target_platform=\"$plat\""
        "target_environment=\"$envv\""
        'ios_deployment_target="15.0"'
        enable_built_in_dns=true
        ios_partition_alloc_enabled=false
      )
      ;;
  esac

  if [ "$hgoos" = windows ]; then
    if command -v sccache >/dev/null 2>&1; then
      args+=("cc_wrapper=\"$(command -v sccache)\"")
    fi
  else
    if command -v ccache >/dev/null 2>&1; then
      args+=("cc_wrapper=\"$(command -v ccache)\"")
    fi
  fi

  local gn_args="${args[*]}"
  local gn="$SRC_ROOT/gn/out/gn"
  [ "$hgoos" = windows ] && gn="$gn.exe"

  log "gn gen $outdir"
  if [ "$hgoos" = windows ]; then
    ( cd "$SRC_ROOT" && DEPOT_TOOLS_WIN_TOOLCHAIN=0 "$gn" gen "$outdir" "--args=$gn_args" )
  else
    ( cd "$SRC_ROOT" && "$gn" gen "$outdir" "--args=$gn_args" )
  fi

  if [ "$T_GOOS" = windows ]; then
    log "ninja -C $outdir cronet"
    ( cd "$SRC_ROOT" && ninja -C "$outdir" cronet )
  else
    log "ninja -C $outdir cronet_static"
    ( cd "$SRC_ROOT" && ninja -C "$outdir" cronet_static )
    if [ "$T_GOOS" = linux ] && [ "$T_LIBC" != musl ]; then
      log "ninja -C $outdir cronet"
      ( cd "$SRC_ROOT" && ninja -C "$outdir" cronet )
    fi
  fi
}

# ---------------------------------------------------------------------------
# Package (copy libs/headers + link flags)
# ---------------------------------------------------------------------------

copy_headers() {
  local inc="$PROJECT_ROOT/include"
  mkdir -p "$inc"
  cp "$SRC_ROOT/components/cronet/native/include/cronet_c.h"      "$inc/cronet_c.h"
  cp "$SRC_ROOT/components/cronet/native/include/cronet_export.h" "$inc/cronet_export.h"
  cp "$SRC_ROOT/components/cronet/native/generated/cronet.idl_c.h" "$inc/cronet.idl_c.h"
  cp "$SRC_ROOT/components/grpc_support/include/bidirectional_stream_c.h" "$inc/bidirectional_stream_c.h"
  log "Copied headers to include/"
}

# Parse the generated cronet_sample.ninja for the system link flags that the
# static library depends on, and write them to link_flags.txt.
write_link_flags() {
  local td="$1" outdir="$2"
  local ninja="$SRC_ROOT/$outdir/obj/components/cronet/cronet_sample.ninja"
  if [ ! -f "$ninja" ]; then
    log "Warning: ninja file missing for $(format_target): $ninja"
    return
  fi

  local libs frameworks ldflags
  libs="$(grep -E '^[[:space:]]*libs[[:space:]]*=' "$ninja" | sed -E 's/^[[:space:]]*libs[[:space:]]*=[[:space:]]*//' | tr '\n' ' ')"
  frameworks="$(grep -E '^[[:space:]]*frameworks[[:space:]]*=' "$ninja" | tail -1 | sed -E 's/^[[:space:]]*frameworks[[:space:]]*=[[:space:]]*//')"
  ldflags="$(grep -E '^[[:space:]]*ldflags[[:space:]]*=' "$ninja" | tail -1 | sed -E 's/^[[:space:]]*ldflags[[:space:]]*=[[:space:]]*//')"

  local out="" tok
  local keep=""
  for tok in $ldflags; do
    case "$tok" in -Wl,-wrap,*) keep="$keep $tok" ;; esac
  done
  keep="${keep# }"
  [ -n "$keep" ] && out="${out}# ldflags\n${keep}\n"

  keep=""
  for tok in $libs; do
    case "$tok" in *.lds) : ;; *) keep="$keep $tok" ;; esac
  done
  keep="${keep# }"
  [ -n "$keep" ] && out="${out}# libs\n${keep}\n"

  keep=""
  set -- $frameworks
  while [ $# -gt 0 ]; do
    if [ "$1" = "-framework" ] && [ $# -ge 2 ]; then
      keep="$keep -framework $2"
      shift 2
    else
      shift
    fi
  done
  keep="${keep# }"
  [ -n "$keep" ] && out="${out}# frameworks\n${keep}\n"

  if [ "$T_GOOS" = linux ] && [ "$T_LIBC" = musl ]; then
    out="${out}# extra\n-static\n"
  fi

  [ -n "$out" ] || return
  printf '%b' "$out" > "$td/link_flags.txt"
  log "Wrote link flags for $(format_target)"
}

package_target() {
  local name td outdir
  name="$(lib_dir_name)"
  td="$PROJECT_ROOT/lib/$name"
  mkdir -p "$td"
  outdir="$(output_dir)"

  if [ "$T_GOOS" = windows ]; then
    local src="$SRC_ROOT/$outdir/cronet.dll"
    if [ -f "$src" ]; then
      cp "$src" "$td/libcronet.dll"
      log "Copied DLL for $T_GOOS/$T_ARCH"
    else
      log "Warning: DLL not found for $T_GOOS/$T_ARCH, skipping"
    fi
    log "Packaged lib/$name"
    return
  fi

  local src_static="$SRC_ROOT/$outdir/obj/components/cronet/libcronet_static.a"
  if [ -f "$src_static" ]; then
    cp "$src_static" "$td/libcronet.a"
    log "Copied static library for $(format_target)"
  else
    log "Warning: static library not found for $(format_target), skipping"
  fi

  if [ "$T_GOOS" = linux ] && [ "$T_LIBC" != musl ]; then
    local src_so="$SRC_ROOT/$outdir/libcronet.so"
    if [ -f "$src_so" ]; then
      cp "$src_so" "$td/libcronet.so"
      log "Copied shared library for $(format_target)"
    fi
  fi

  write_link_flags "$td" "$outdir"
  log "Packaged lib/$name"
}

# ---------------------------------------------------------------------------
# env
# ---------------------------------------------------------------------------

shell_quote() {
  local s="$1"
  if [ "$EXPORT" = 1 ] && printf '%s' "$s" | grep -qE '[[:space:]"'\''\\$]'; then
    printf '"%s"' "$(printf '%s' "$s" | sed 's/"/\\"/g')"
  else
    printf '%s' "$s"
  fi
}

print_env() {
  [ "$T_GOOS" = windows ] && die "env command is not supported for Windows (use the prebuilt DLL)"

  local prefix=""
  [ "$EXPORT" = 1 ] && prefix="export "

  if [ "$T_GOOS" = linux ]; then
    local ld="-fuse-ld=lld"
    case "$T_ARCH" in
      386|arm|loong64|mipsle|mips64le) ld="$ld -Wl,-no-pie" ;;
    esac
    if { [ "$T_ARCH" = mipsle ] || [ "$T_ARCH" = mips64le ]; } && [ "$T_LIBC" != musl ]; then
      ld="$ld -Wl,-z,execstack"
    fi
    printf '%sCGO_LDFLAGS=%s\n' "$prefix" "$(shell_quote "$ld")"

    local clang="$SRC_ROOT/third_party/llvm-build/Release+Asserts/bin/clang"
    local ct sp
    ct="$(clang_target)"
    sp="$(sysroot_path)"
    printf '%sCC=%s\n'  "$prefix" "$(shell_quote "$clang --target=$ct --sysroot=$sp")"
    printf '%sCXX=%s\n' "$prefix" "$(shell_quote "$clang++ --target=$ct --sysroot=$sp")"
    printf '%sQEMU_LD_PREFIX=%s\n' "$prefix" "$sp"
  fi
}

# ---------------------------------------------------------------------------
# Target list expansion
# ---------------------------------------------------------------------------

expand_targets() {
  local spec="$1"
  if [ "$spec" = all ]; then
    echo "$ALL_SPECS"
  elif [ -z "$spec" ]; then
    echo ""    # single host target (empty string)
  else
    echo "$spec" | tr ',' ' '
  fi
}

# ---------------------------------------------------------------------------
# Main
# ---------------------------------------------------------------------------

main() {
  [ $# -ge 1 ] || die "usage: build-naive.sh <build|package|download-toolchain|env> [--target LIST] [--libc glibc|musl] [--export]"
  local cmd="$1"; shift

  local target="" libc=""
  EXPORT=0
  while [ $# -gt 0 ]; do
    case "$1" in
      --target) target="$2"; shift 2 ;;
      --target=*) target="${1#--target=}"; shift ;;
      -t) target="$2"; shift 2 ;;
      --libc) libc="$2"; shift 2 ;;
      --libc=*) libc="${1#--libc=}"; shift ;;
      --export) EXPORT=1; shift ;;
      "") shift ;;
      *) die "unknown argument: $1" ;;
    esac
  done

  case "$cmd" in
    build)
      local specs; specs="$(expand_targets "$target")"
      if [ -z "$target" ]; then
        resolve_target "" "$libc"; log "Building $(format_target)..."; compile_target
      else
        for s in $specs; do resolve_target "$s" "$libc"; log "Building $(format_target)..."; compile_target; done
      fi
      log "Build complete!"
      ;;
    package)
      copy_headers
      local specs; specs="$(expand_targets "$target")"
      if [ -z "$target" ]; then
        resolve_target "" "$libc"; package_target
      else
        for s in $specs; do resolve_target "$s" "$libc"; package_target; done
      fi
      log "Package complete!"
      ;;
    download-toolchain)
      local specs; specs="$(expand_targets "$target")"
      if [ -z "$target" ]; then
        resolve_target "" "$libc"; log "Downloading toolchain for $(format_target)..."; run_get_clang
      else
        for s in $specs; do resolve_target "$s" "$libc"; log "Downloading toolchain for $(format_target)..."; run_get_clang; done
      fi
      log "Toolchain download complete!"
      ;;
    env)
      # env requires exactly one target
      case "$target" in
        *,*) die "env requires exactly one target" ;;
      esac
      resolve_target "$target" "$libc"
      print_env
      ;;
    *)
      die "unknown command: $cmd"
      ;;
  esac
}

main "$@"
