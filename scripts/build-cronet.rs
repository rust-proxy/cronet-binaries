#!/usr/bin/env -S cargo +nightly -Zscript
---
[package]
edition = "2021"
---
//! build-cronet — cronet library build orchestration (single-file cargo script).
//!
//! A faithful port of the former Go/bash tool. Depends only on the Rust
//! standard library, so `cargo -Zscript` compiles it with no network access.
//! It drives naiveproxy's get-clang.sh, then `gn gen` + `ninja`, and packages
//! the resulting .a/.so/.dll into lib/ + include/.
//!
//! Subcommands:
//!   build              gn gen + ninja  -> libcronet.{a,so,dll}
//!   package            copy libs/headers into lib/ + include/, dump link flags
//!   download-toolchain download clang + sysroot without building
//!   env                print CC/CXX/CGO_LDFLAGS for cross-compiling consumers
//!   print-config       (debug) print the resolved config + gn args, run nothing
//!
//! Flags: --target <list> (comma separated; empty = host, "all" = every target)
//!        --libc <glibc|musl>   musl => static OpenWrt/musl Linux build
//!        --export              (env only) prefix output with `export `

use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{exit, Command};

const ALL_SPECS: &[&str] = &[
    "linux/amd64", "linux/arm64", "linux/386", "linux/arm", "linux/loong64",
    "linux/mipsle", "linux/mips64le", "linux/riscv64",
    "darwin/amd64", "darwin/arm64", "windows/amd64", "windows/arm64",
    "ios/arm64", "ios/arm64/simulator", "ios/amd64",
    "tvos/arm64", "tvos/arm64/simulator", "tvos/amd64",
    "android/arm64", "android/amd64", "android/arm", "android/386",
];

fn log(msg: &str) {
    eprintln!("[build] {msg}");
}

fn die(msg: &str) -> ! {
    eprintln!("[build] ERROR: {msg}");
    exit(1);
}

// ---------------------------------------------------------------------------
// Host detection
// ---------------------------------------------------------------------------

fn host_goos() -> &'static str {
    match env::consts::OS {
        "linux" => "linux",
        "macos" => "darwin",
        "windows" => "windows",
        _ => "unknown",
    }
}

fn host_goarch() -> &'static str {
    match env::consts::ARCH {
        "x86_64" => "amd64",
        "aarch64" => "arm64",
        "x86" => "386",
        "arm" => "arm",
        "loongarch64" => "loong64",
        "riscv64" => "riscv64",
        "mips64" => "mips64le",
        "mips" => "mipsle",
        other => other,
    }
}

fn arch_to_cpu(arch: &str) -> String {
    match arch {
        "amd64" => "x64",
        "arm64" => "arm64",
        "386" => "x86",
        "arm" => "arm",
        "loong64" => "loong64",
        "mipsle" => "mipsel",
        "mips64le" => "mips64el",
        other => other,
    }
    .to_string()
}

// ---------------------------------------------------------------------------
// Target resolution
// ---------------------------------------------------------------------------

#[derive(Clone, Default)]
struct Target {
    os: String,          // gn target_os: linux, mac, win, android, ios, openwrt
    cpu: String,         // gn target_cpu: x64, arm64, x86, arm, ...
    goos: String,        // Go GOOS
    arch: String,        // Go GOARCH
    libc: String,        // "" or "musl"
    platform: String,    // Apple: iphoneos | tvos
    environment: String, // Apple: device | simulator
}

fn resolve_target(spec: &str, libc: &str) -> Target {
    let owned;
    let spec = if spec.is_empty() {
        owned = format!("{}/{}", host_goos(), host_goarch());
        owned.as_str()
    } else {
        spec
    };

    let parts: Vec<&str> = spec.split('/').collect();
    if parts.len() < 2 || parts.len() > 3 {
        die(&format!("invalid target format: {spec} (expected os/arch or os/arch/variant)"));
    }
    let goos = parts[0];
    let goarch = parts[1];
    let variant = if parts.len() == 3 { parts[2] } else { "" };

    if goos == "ios" || goos == "tvos" {
        return resolve_apple(goos, goarch, variant);
    }

    let mut t = Target::default();
    t.goos = goos.to_string();
    t.arch = goarch.to_string();
    t.cpu = arch_to_cpu(goarch);
    t.os = match goos {
        "linux" => "linux",
        "darwin" => "mac",
        "windows" => "win",
        "android" => "android",
        _ => die(&format!("unsupported target: {spec}")),
    }
    .to_string();

    if !libc.is_empty() && libc != "glibc" {
        if libc != "musl" {
            die(&format!("invalid libc: {libc} (expected glibc or musl)"));
        }
        if goos != "linux" {
            die(&format!("--libc=musl is only supported for Linux targets, not {goos}"));
        }
        t.os = "openwrt".to_string();
        t.libc = "musl".to_string();
    }
    t
}

fn resolve_apple(goos: &str, goarch: &str, variant: &str) -> Target {
    let mut t = Target::default();
    t.goos = "ios".to_string();
    t.os = "ios".to_string();
    t.arch = goarch.to_string();
    t.cpu = arch_to_cpu(goarch);
    t.platform = if goos == "tvos" { "tvos" } else { "iphoneos" }.to_string();
    t.environment = if variant == "simulator" {
        "simulator"
    } else if goarch == "amd64" {
        "simulator"
    } else {
        "device"
    }
    .to_string();
    t
}

fn format_target(t: &Target) -> String {
    if !t.platform.is_empty() {
        let pn = if t.platform == "tvos" { "tvOS" } else { "iOS" };
        if t.environment == "simulator" {
            format!("{pn} Simulator {}", t.arch)
        } else {
            format!("{pn} {}", t.arch)
        }
    } else if t.libc == "musl" {
        format!("{}/{} (musl)", t.goos, t.arch)
    } else {
        format!("{}/{}", t.goos, t.arch)
    }
}

fn output_dir(t: &Target) -> String {
    if !t.platform.is_empty() {
        let mut d = format!("out/cronet-{}-{}", t.platform, t.cpu);
        if t.environment == "simulator" {
            d.push_str("-simulator");
        }
        d
    } else {
        format!("out/cronet-{}-{}", t.os, t.cpu)
    }
}

fn lib_dir_name(t: &Target) -> String {
    let osn = if t.platform == "tvos" { "tvos".to_string() } else { t.goos.clone() };
    let mut n = format!("{osn}_{}", t.arch);
    if t.environment == "simulator" {
        n.push_str("_simulator");
    }
    if t.libc == "musl" {
        n.push_str("_musl");
    }
    n
}

// ---------------------------------------------------------------------------
// OpenWrt / musl + sysroot + clang config
// ---------------------------------------------------------------------------

struct Openwrt {
    target: &'static str,
    subtarget: &'static str,
    arch: &'static str,
    release: &'static str,
    gcc: &'static str,
    extra: Vec<&'static str>,
}

fn openwrt_config(t: &Target) -> Openwrt {
    match t.cpu.as_str() {
        "x64" => Openwrt { target: "x86", subtarget: "64", arch: "x86_64", release: "23.05.5", gcc: "12.3.0", extra: vec![] },
        "arm64" => Openwrt { target: "armsr", subtarget: "armv8", arch: "aarch64", release: "23.05.5", gcc: "12.3.0", extra: vec![] },
        "x86" => Openwrt { target: "x86", subtarget: "generic", arch: "i386_pentium4", release: "23.05.5", gcc: "12.3.0", extra: vec![] },
        "arm" => Openwrt { target: "armsr", subtarget: "armv7", arch: "arm_cortex-a15_neon-vfpv4", release: "23.05.5", gcc: "12.3.0", extra: vec![] },
        "loong64" => Openwrt { target: "loongarch64", subtarget: "generic", arch: "loongarch64", release: "24.10.5", gcc: "13.3.0", extra: vec![] },
        "mipsel" => Openwrt { target: "ramips", subtarget: "rt305x", arch: "mipsel_24kc", release: "23.05.5", gcc: "12.3.0", extra: vec![r#"mips_float_abi="soft""#, r#"mips_arch_variant="r2""#] },
        "riscv64" => Openwrt { target: "sifiveu", subtarget: "generic", arch: "riscv64", release: "23.05.5", gcc: "12.3.0", extra: vec![] },
        other => die(&format!("unsupported CPU for musl: {other}")),
    }
}

fn glibc_sysroot_path(src: &Path, cpu: &str) -> PathBuf {
    let (arch, rel) = match cpu {
        "x64" => ("amd64", "bullseye"),
        "arm64" => ("arm64", "bullseye"),
        "x86" => ("i386", "bullseye"),
        "arm" => ("armhf", "bullseye"),
        "loong64" => ("loong64", "sid"),
        "mipsel" => ("mipsel", "bullseye"),
        "mips64el" => ("mips64el", "bullseye"),
        "riscv64" => ("riscv64", "trixie"),
        other => die(&format!("no sysroot for cpu: {other}")),
    };
    src.join(format!("out/sysroot-build/{rel}/{rel}_{arch}_staging"))
}

fn sysroot_path(src: &Path, t: &Target) -> PathBuf {
    if t.libc == "musl" {
        let ow = openwrt_config(t);
        src.join(format!("out/sysroot-build/openwrt/{}/{}", ow.release, ow.arch))
    } else {
        glibc_sysroot_path(src, &t.cpu)
    }
}

fn clang_target(t: &Target) -> &'static str {
    if t.libc == "musl" {
        return match t.cpu.as_str() {
            "x64" => "x86_64-openwrt-linux-musl",
            "arm64" => "aarch64-openwrt-linux-musl",
            "x86" => "i486-openwrt-linux-musl",
            "arm" => "arm-openwrt-linux-musleabi",
            "loong64" => "loongarch64-openwrt-linux-musl",
            "mipsel" => "mipsel-openwrt-linux-musl",
            "riscv64" => "riscv64-openwrt-linux-musl",
            _ => "",
        };
    }
    match t.cpu.as_str() {
        "x64" => "x86_64-linux-gnu",
        "arm64" => "aarch64-linux-gnu",
        "x86" => "i686-linux-gnu",
        "arm" => "arm-linux-gnueabihf",
        "loong64" => "loongarch64-linux-gnu",
        "mipsel" => "mipsel-linux-gnu",
        "mips64el" => "mips64el-linux-gnuabi64",
        "riscv64" => "riscv64-linux-gnu",
        _ => "",
    }
}

// ---------------------------------------------------------------------------
// Process helpers
// ---------------------------------------------------------------------------

fn run(dir: &Path, program: &str, args: &[&str], envs: &[(&str, &str)]) {
    let mut c = Command::new(program);
    c.current_dir(dir).args(args);
    for (k, v) in envs {
        c.env(k, v);
    }
    let status = c
        .status()
        .unwrap_or_else(|e| die(&format!("failed to spawn {program}: {e}")));
    if !status.success() {
        die(&format!("command failed: {program} {}", args.join(" ")));
    }
}

/// Locate a real bash to run get-clang.sh. On Windows a bare `bash` resolves to
/// C:\Windows\System32\bash.exe (WSL), which has no distro on CI runners, so we
/// must use Git's bash explicitly.
fn bash_program() -> String {
    if cfg!(windows) {
        let candidates = [
            r"C:\Program Files\Git\bin\bash.exe",
            r"C:\Program Files\Git\usr\bin\bash.exe",
            r"C:\Program Files (x86)\Git\bin\bash.exe",
        ];
        for c in candidates {
            if Path::new(c).is_file() {
                return c.to_string();
            }
        }
        if let Some(path) = env::var_os("PATH") {
            for dir in env::split_paths(&path) {
                let lower = dir.to_string_lossy().to_lowercase();
                if lower.contains(r"\windows\system32") || lower.contains(r"\windows\sysnative") {
                    continue; // skip WSL's bash
                }
                let cand = dir.join("bash.exe");
                if cand.is_file() {
                    return cand.to_string_lossy().to_string();
                }
            }
        }
    }
    "bash".to_string()
}

fn find_in_path(name: &str) -> Option<PathBuf> {
    let exts: &[&str] = if cfg!(windows) { &["", ".exe", ".bat", ".cmd"] } else { &[""] };
    let path = env::var_os("PATH")?;
    for dir in env::split_paths(&path) {
        for ext in exts {
            let cand = dir.join(format!("{name}{ext}"));
            if cand.is_file() {
                return Some(cand);
            }
        }
    }
    None
}

// ---------------------------------------------------------------------------
// Toolchain
// ---------------------------------------------------------------------------

fn run_get_clang(src: &Path, t: &Target) {
    let bash = bash_program();

    if host_goos() == "linux" && (t.os == "linux" || t.os == "android" || t.os == "openwrt") {
        let hcpu = arch_to_cpu(host_goarch());
        let host_flags = format!("target_os=\"linux\" target_cpu=\"{hcpu}\"");
        log(&format!("get-clang.sh (host sysroot) EXTRA_FLAGS={host_flags}"));
        run(src, &bash, &["./get-clang.sh"], &[("EXTRA_FLAGS", &host_flags)]);

        let host_src = glibc_sysroot_path(src, &hcpu);
        let host_dst = src.join("build/linux/debian_bullseye_amd64-sysroot");
        if !host_dst.exists() {
            log(&format!("symlink host sysroot {} -> {}", host_dst.display(), host_src.display()));
            symlink_dir(&host_src, &host_dst);
        }
    }

    let extra = format!("target_os=\"{}\" target_cpu=\"{}\"", t.os, t.cpu);
    if t.os == "openwrt" {
        let ow = openwrt_config(t);
        let owf = format!(
            "target=\"{}\" subtarget=\"{}\" arch=\"{}\" release=\"{}\" gcc_ver=\"{}\"",
            ow.target, ow.subtarget, ow.arch, ow.release, ow.gcc
        );
        log(&format!("get-clang.sh EXTRA_FLAGS={extra} OPENWRT_FLAGS={owf}"));
        run(src, &bash, &["./get-clang.sh"], &[("EXTRA_FLAGS", &extra), ("OPENWRT_FLAGS", &owf)]);
    } else {
        log(&format!("get-clang.sh EXTRA_FLAGS={extra}"));
        run(src, &bash, &["./get-clang.sh"], &[("EXTRA_FLAGS", &extra)]);
    }
}

#[cfg(unix)]
fn symlink_dir(src: &Path, dst: &Path) {
    std::os::unix::fs::symlink(src, dst).unwrap_or_else(|e| die(&format!("failed to create symlink: {e}")));
}

#[cfg(not(unix))]
fn symlink_dir(_src: &Path, _dst: &Path) {
    // Host-sysroot symlinking only happens on a Linux host; unreachable elsewhere.
}

// ---------------------------------------------------------------------------
// gn args + build
// ---------------------------------------------------------------------------

fn build_gn_args(src: &Path, t: &Target) -> Vec<String> {
    let base = [
        "is_official_build=true",
        "is_debug=false",
        "is_clang=true",
        "use_clang_modules=false",
        "use_thin_lto=false",
        "fatal_linker_warnings=false",
        "treat_warnings_as_errors=false",
        "is_cronet_build=true",
        "use_udev=false",
        "use_aura=false",
        "use_ozone=false",
        "use_gio=false",
        "use_glib=false",
        "use_kerberos=false",
        "disable_zstd_filter=false",
        "enable_reporting=false",
        "enable_bracketed_proxy_uris=true",
        "enable_quic_proxy_support=true",
        "use_nss_certs=false",
        "enable_backup_ref_ptr_support=false",
        "enable_dangling_raw_ptr_checks=false",
        "exclude_unwind_tables=true",
        "enable_resource_allowlist_generation=false",
        "symbol_level=0",
        "enable_dsyms=false",
        "optimize_for_size=true",
    ];
    let mut args: Vec<String> = base.iter().map(|s| s.to_string()).collect();
    args.push(format!("target_os=\"{}\"", t.os));
    args.push(format!("target_cpu=\"{}\"", t.cpu));

    match t.os.as_str() {
        "mac" => args.push("use_sysroot=false".to_string()),
        "linux" => {
            let sp = sysroot_path(src, t);
            let rel = sp
                .strip_prefix(src)
                .map(|p| p.to_string_lossy().replace('\\', "/"))
                .unwrap_or_else(|_| sp.to_string_lossy().to_string());
            args.push("use_sysroot=true".to_string());
            args.push(format!("target_sysroot=\"//{rel}\""));
            if t.cpu == "x64" {
                args.push("use_cfi_icall=false".to_string());
                args.push("is_cfi=false".to_string());
            }
        }
        "openwrt" => {
            let ow = openwrt_config(t);
            let sd = format!("out/sysroot-build/openwrt/{}/{}", ow.release, ow.arch);
            args.push("use_sysroot=true".to_string());
            args.push(format!("target_sysroot=\"//{sd}\""));
            args.push("build_static=true".to_string());
            args.push("use_allocator_shim=false".to_string());
            args.push("use_partition_alloc=false".to_string());
            for e in ow.extra {
                args.push(e.to_string());
            }
            if t.cpu == "x64" {
                args.push("use_cfi_icall=false".to_string());
                args.push("is_cfi=false".to_string());
            }
        }
        "win" => args.push("use_sysroot=false".to_string()),
        "android" => {
            args.push("use_sysroot=false".to_string());
            args.push("default_min_sdk_version=23".to_string());
        }
        "ios" => {
            let plat = if t.platform.is_empty() { "iphoneos" } else { &t.platform };
            let envv = if t.environment.is_empty() { "device" } else { &t.environment };
            args.push("use_sysroot=false".to_string());
            args.push("ios_enable_code_signing=false".to_string());
            args.push(format!("target_platform=\"{plat}\""));
            args.push(format!("target_environment=\"{envv}\""));
            args.push("ios_deployment_target=\"15.0\"".to_string());
            args.push("enable_built_in_dns=true".to_string());
            args.push("ios_partition_alloc_enabled=false".to_string());
        }
        _ => {}
    }

    let wrapper = if host_goos() == "windows" { "sccache" } else { "ccache" };
    if let Some(p) = find_in_path(wrapper) {
        args.push(format!("cc_wrapper=\"{}\"", p.to_string_lossy()));
    }

    args
}

fn compile_target(src: &Path, t: &Target) {
    run_get_clang(src, t);

    let outdir = output_dir(t);
    let gn_args = build_gn_args(src, t).join(" ");

    let gn = if host_goos() == "windows" {
        src.join("gn/out/gn.exe")
    } else {
        src.join("gn/out/gn")
    };
    let gn_str = gn.to_string_lossy().to_string();

    log(&format!("gn gen {outdir}"));
    let args_flag = format!("--args={gn_args}");
    let gen_args = ["gen", outdir.as_str(), args_flag.as_str()];
    if host_goos() == "windows" {
        run(src, &gn_str, &gen_args, &[("DEPOT_TOOLS_WIN_TOOLCHAIN", "0")]);
    } else {
        run(src, &gn_str, &gen_args, &[]);
    }

    if t.goos == "windows" {
        log(&format!("ninja -C {outdir} cronet"));
        run(src, "ninja", &["-C", &outdir, "cronet"], &[]);
    } else {
        log(&format!("ninja -C {outdir} cronet_static"));
        run(src, "ninja", &["-C", &outdir, "cronet_static"], &[]);
        if t.goos == "linux" && t.libc != "musl" {
            log(&format!("ninja -C {outdir} cronet"));
            run(src, "ninja", &["-C", &outdir, "cronet"], &[]);
        }
    }
}

// ---------------------------------------------------------------------------
// Package
// ---------------------------------------------------------------------------

fn copy_file(srcf: &Path, dst: &Path) {
    if let Some(p) = dst.parent() {
        let _ = fs::create_dir_all(p);
    }
    fs::copy(srcf, dst)
        .unwrap_or_else(|e| die(&format!("failed to copy {} -> {}: {e}", srcf.display(), dst.display())));
}

fn copy_headers(root: &Path, src: &Path) {
    let inc = root.join("include");
    let _ = fs::create_dir_all(&inc);
    let headers = [
        ("components/cronet/native/include/cronet_c.h", "cronet_c.h"),
        ("components/cronet/native/include/cronet_export.h", "cronet_export.h"),
        ("components/cronet/native/generated/cronet.idl_c.h", "cronet.idl_c.h"),
        ("components/grpc_support/include/bidirectional_stream_c.h", "bidirectional_stream_c.h"),
    ];
    for (s, d) in headers {
        copy_file(&src.join(s), &inc.join(d));
    }
    log("Copied headers to include/");
}

struct LinkFlags {
    libs: Vec<String>,
    frameworks: Vec<String>,
    ldflags: Vec<String>,
}

fn ninja_kv<'a>(line: &'a str, key: &str) -> Option<&'a str> {
    let rest = line.trim_start().strip_prefix(key)?;
    let rest = rest.trim_start().strip_prefix('=')?;
    Some(rest.trim_start())
}

fn parse_frameworks(input: &str) -> Vec<String> {
    let toks: Vec<&str> = input.split_whitespace().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < toks.len() {
        if toks[i] == "-framework" && i + 1 < toks.len() {
            out.push(format!("-framework {}", toks[i + 1]));
            i += 2;
        } else {
            i += 1;
        }
    }
    out
}

fn extract_link_flags(src: &Path, outdir: &str) -> Option<LinkFlags> {
    let ninja = src.join(outdir).join("obj/components/cronet/cronet_sample.ninja");
    let content = fs::read_to_string(&ninja).ok()?;
    let mut lf = LinkFlags { libs: vec![], frameworks: vec![], ldflags: vec![] };
    for line in content.lines() {
        if let Some(rest) = ninja_kv(line, "libs") {
            for tok in rest.split_whitespace() {
                if !tok.ends_with(".lds") {
                    lf.libs.push(tok.to_string());
                }
            }
        } else if let Some(rest) = ninja_kv(line, "frameworks") {
            lf.frameworks = parse_frameworks(rest);
        } else if let Some(rest) = ninja_kv(line, "ldflags") {
            lf.ldflags = rest
                .split_whitespace()
                .filter(|tok| tok.starts_with("-Wl,-wrap,"))
                .map(String::from)
                .collect();
        }
    }
    Some(lf)
}

fn write_link_flags(src: &Path, t: &Target, td: &Path, outdir: &str) {
    let lf = match extract_link_flags(src, outdir) {
        Some(x) => x,
        None => {
            log(&format!("Warning: could not extract link flags for {}", format_target(t)));
            return;
        }
    };
    let mut out = String::new();
    if !lf.ldflags.is_empty() {
        out.push_str(&format!("# ldflags\n{}\n", lf.ldflags.join(" ")));
    }
    if !lf.libs.is_empty() {
        out.push_str(&format!("# libs\n{}\n", lf.libs.join(" ")));
    }
    if !lf.frameworks.is_empty() {
        out.push_str(&format!("# frameworks\n{}\n", lf.frameworks.join(" ")));
    }
    if t.goos == "linux" && t.libc == "musl" {
        out.push_str("# extra\n-static\n");
    }
    if out.is_empty() {
        return;
    }
    fs::write(td.join("link_flags.txt"), out)
        .unwrap_or_else(|e| die(&format!("failed to write link_flags.txt: {e}")));
    log(&format!("Wrote link flags for {}", format_target(t)));
}

fn package_target(root: &Path, src: &Path, t: &Target) {
    let name = lib_dir_name(t);
    let td = root.join("lib").join(&name);
    let _ = fs::create_dir_all(&td);
    let outdir = output_dir(t);

    if t.goos == "windows" {
        let s = src.join(&outdir).join("cronet.dll");
        if s.is_file() {
            copy_file(&s, &td.join("libcronet.dll"));
            log(&format!("Copied DLL for {}/{}", t.goos, t.arch));
        } else {
            log(&format!("Warning: DLL not found for {}/{}, skipping", t.goos, t.arch));
        }
        log(&format!("Packaged lib/{name}"));
        return;
    }

    let ss = src.join(&outdir).join("obj/components/cronet/libcronet_static.a");
    if ss.is_file() {
        copy_file(&ss, &td.join("libcronet.a"));
        log(&format!("Copied static library for {}", format_target(t)));
    } else {
        log(&format!("Warning: static library not found for {}, skipping", format_target(t)));
    }

    if t.goos == "linux" && t.libc != "musl" {
        let so = src.join(&outdir).join("libcronet.so");
        if so.is_file() {
            copy_file(&so, &td.join("libcronet.so"));
            log(&format!("Copied shared library for {}", format_target(t)));
        }
    }

    write_link_flags(src, t, &td, &outdir);
    log(&format!("Packaged lib/{name}"));
}

// ---------------------------------------------------------------------------
// env
// ---------------------------------------------------------------------------

fn shell_quote(s: &str, export: bool) -> String {
    if export && s.chars().any(|c| " \t\n\"'\\$".contains(c)) {
        format!("\"{}\"", s.replace('"', "\\\""))
    } else {
        s.to_string()
    }
}

fn print_env(src: &Path, t: &Target, export: bool) {
    if t.goos == "windows" {
        die("env command is not supported for Windows (use the prebuilt DLL)");
    }
    let prefix = if export { "export " } else { "" };

    if t.goos == "linux" {
        let mut ld = vec!["-fuse-ld=lld".to_string()];
        if matches!(t.arch.as_str(), "386" | "arm" | "loong64" | "mipsle" | "mips64le") {
            ld.push("-Wl,-no-pie".to_string());
        }
        if (t.arch == "mipsle" || t.arch == "mips64le") && t.libc != "musl" {
            ld.push("-Wl,-z,execstack".to_string());
        }
        println!("{prefix}CGO_LDFLAGS={}", shell_quote(&ld.join(" "), export));

        let clang = src.join("third_party/llvm-build/Release+Asserts/bin/clang");
        let ct = clang_target(t);
        let sp = sysroot_path(src, t);
        let cc = format!("{} --target={ct} --sysroot={}", clang.display(), sp.display());
        let cxx = format!("{}++ --target={ct} --sysroot={}", clang.display(), sp.display());
        println!("{prefix}CC={}", shell_quote(&cc, export));
        println!("{prefix}CXX={}", shell_quote(&cxx, export));
        println!("{prefix}QEMU_LD_PREFIX={}", sp.display());
    }
}

fn print_config(src: &Path, t: &Target) {
    let opt = |s: &str| if s.is_empty() { "-".to_string() } else { s.to_string() };
    println!(
        "spec: os={} cpu={} goos={} arch={} libc={} platform={} env={}",
        t.os, t.cpu, t.goos, t.arch, opt(&t.libc), opt(&t.platform), opt(&t.environment)
    );
    println!("output_dir:   {}", output_dir(t));
    println!("lib_dir_name: {}", lib_dir_name(t));
    println!("gn_args:      {}", build_gn_args(src, t).join(" "));
}

// ---------------------------------------------------------------------------
// Project root + main
// ---------------------------------------------------------------------------

fn project_root() -> PathBuf {
    let cwd = env::current_dir().unwrap_or_else(|e| die(&format!("failed to get working directory: {e}")));
    let mut d = cwd.as_path();
    loop {
        if d.join("naiveproxy").is_dir() {
            return d.to_path_buf();
        }
        match d.parent() {
            Some(p) => d = p,
            None => break,
        }
    }
    cwd
}

fn expand_targets(target: &str) -> Vec<String> {
    if target == "all" {
        ALL_SPECS.iter().map(|s| s.to_string()).collect()
    } else if target.is_empty() {
        vec![String::new()]
    } else {
        target.split(',').map(|s| s.trim().to_string()).collect()
    }
}

fn main() {
    let raw: Vec<String> = env::args().skip(1).skip_while(|s| s == "--").collect();
    if raw.is_empty() {
        die("usage: build-cronet <build|package|download-toolchain|env|print-config> [--target LIST] [--libc glibc|musl] [--export]");
    }
    let cmd = raw[0].clone();

    let mut target = String::new();
    let mut libc = String::new();
    let mut export = false;
    let mut i = 1;
    while i < raw.len() {
        let a = &raw[i];
        if a == "--target" || a == "-t" {
            i += 1;
            target = raw.get(i).cloned().unwrap_or_default();
        } else if let Some(v) = a.strip_prefix("--target=") {
            target = v.to_string();
        } else if a == "--libc" {
            i += 1;
            libc = raw.get(i).cloned().unwrap_or_default();
        } else if let Some(v) = a.strip_prefix("--libc=") {
            libc = v.to_string();
        } else if a == "--export" {
            export = true;
        } else if !a.is_empty() {
            die(&format!("unknown argument: {a}"));
        }
        i += 1;
    }

    let root = project_root();
    let src = root.join("naiveproxy").join("src");
    let specs = expand_targets(&target);

    match cmd.as_str() {
        "build" => {
            for s in &specs {
                let t = resolve_target(s, &libc);
                log(&format!("Building {}...", format_target(&t)));
                compile_target(&src, &t);
            }
            log("Build complete!");
        }
        "package" => {
            copy_headers(&root, &src);
            for s in &specs {
                let t = resolve_target(s, &libc);
                package_target(&root, &src, &t);
            }
            log("Package complete!");
        }
        "download-toolchain" => {
            for s in &specs {
                let t = resolve_target(s, &libc);
                log(&format!("Downloading toolchain for {}...", format_target(&t)));
                run_get_clang(&src, &t);
            }
            log("Toolchain download complete!");
        }
        "env" => {
            if target.contains(',') {
                die("env requires exactly one target");
            }
            let t = resolve_target(&target, &libc);
            print_env(&src, &t, export);
        }
        "print-config" => {
            for s in &specs {
                let t = resolve_target(s, &libc);
                print_config(&src, &t);
            }
        }
        other => die(&format!("unknown command: {other}")),
    }
}
