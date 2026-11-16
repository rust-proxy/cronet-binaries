//! Keep Cronet's private TLS implementation separate from the application's.
//! Chromium forward-declares BoringSSL types and has identifiers such as SHA256
//! in its own namespaces, so plain function-name macros are not sufficient.

use std::collections::BTreeSet;
use std::error::Error;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

type Result<T> = std::result::Result<T, Box<dyn Error>>;

fn nm_path(src: &Path) -> PathBuf {
    src.join("third_party/llvm-build/Release+Asserts/bin")
        .join(if cfg!(windows) {
            "llvm-nm.exe"
        } else {
            "llvm-nm"
        })
}

pub fn ensure_tools(src: &Path) -> Result<()> {
    if !nm_path(src).is_file() {
        // Chromium ships llvm-nm in the supplementary objdump package on
        // macOS and Windows. update.py selects the same pinned LLVM revision.
        let status = Command::new(if cfg!(windows) { "python" } else { "python3" })
            .current_dir(src)
            .args(["tools/clang/scripts/update.py", "--package=objdump"])
            .status()?;
        if !status.success() || !nm_path(src).is_file() {
            return Err("failed to install Chromium's pinned llvm-nm".into());
        }
    }
    Ok(())
}

fn write_changed(path: &Path, contents: &str) -> Result<()> {
    if fs::read(path).ok().as_deref() != Some(contents.as_bytes()) {
        fs::write(path, contents)?;
    }
    Ok(())
}

/// Apply the GN integration in the build checkout; no native source fork is
/// needed. Exact anchors make an incompatible upstream update fail explicitly.
pub fn patch_gn(src: &Path) -> Result<()> {
    let path = src.join("third_party/boringssl/BUILD.gn");
    let mut text = fs::read_to_string(&path)?.replace("\r\n", "\n");
    if text.contains("cronet_boringssl_prefix") {
        return Ok(());
    }
    let replacements = [
        (
            "# Config for us and everybody else depending on BoringSSL.",
            "declare_args() {\n  cronet_boringssl_prefix = false\n}\n\n# Config for us and everybody else depending on BoringSSL.",
        ),
        (
            "config(\"external_config\") {\n  include_dirs = [ \"src/include\" ]",
            r#"config("external_config") {
  include_dirs = [ "src/include" ]
  defines = []
  if (cronet_boringssl_prefix && current_toolchain == default_toolchain) {
    include_dirs += [ "$root_gen_dir/cronet_boringssl" ]
    # Rename the outer namespace too: Chromium forward-declares bssl types.
    defines += [ "bssl=cronet_bssl" ]
    asmflags = [ "-DBORINGSSL_PREFIX=cronet_bssl" ]
    prefix_header = rebase_path("$root_gen_dir/cronet_boringssl/boringssl_prefix_symbols.h", root_build_dir)
    if (is_win) {
      cflags = [ "/FI" + prefix_header ]
    } else {
      cflags = [ "-include", prefix_header ]
    }
  }"#,
        ),
        (
            "defines = [ \"BORINGSSL_SHARED_LIBRARY\" ]",
            "defines += [ \"BORINGSSL_SHARED_LIBRARY\" ]",
        ),
        (
            "  source_set(\"boringssl_asm\") {",
            "  source_set(\"boringssl_asm\") {\n    configs += [ \":external_config\" ]",
        ),
        (
            "  nasm_assemble(\"boringssl_asm\") {",
            "  nasm_assemble(\"boringssl_asm\") {\n    if (cronet_boringssl_prefix && current_toolchain == default_toolchain) {\n      include_dirs = [ \"$root_gen_dir/cronet_boringssl\" ]\n      defines = [ \"BORINGSSL_PREFIX=cronet_bssl\" ]\n    }",
        ),
    ];
    for (before, after) in replacements {
        if !text.contains(before) {
            return Err(format!("BoringSSL GN integration anchor missing: {before}").into());
        }
        text = text.replace(before, after);
    }
    write_changed(&path, &text)
}

fn llvm_nm(src: &Path, objects: &[PathBuf]) -> Result<String> {
    let tool = nm_path(src);
    let result = Command::new(tool)
        .args(["--defined-only", "--extern-only", "--format=posix"])
        .args(objects)
        .output()?;
    if !result.status.success() {
        return Err(format!(
            "llvm-nm failed: {}",
            String::from_utf8_lossy(&result.stderr)
        )
        .into());
    }
    Ok(String::from_utf8(result.stdout)?)
}

fn c_identifier(name: &str) -> bool {
    name.as_bytes()
        .first()
        .is_some_and(|c| c.is_ascii_alphabetic() || *c == b'_')
        && name.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'_')
}

fn c_symbols(text: &str, apple: bool, data_only: bool) -> BTreeSet<String> {
    text.lines()
        .filter_map(|line| {
            let mut fields = line.split_whitespace();
            let name = fields.next()?;
            let kind = fields.next()?;
            if kind.len() != 1 || (data_only && matches!(kind, "T" | "W")) {
                return None;
            }
            if apple {
                Some(name.strip_prefix('_').unwrap_or(name))
            } else {
                Some(name)
            }
        })
        .filter(|name| c_identifier(name) && !name.starts_with("_Z"))
        .map(String::from)
        .collect()
}

fn struct_tags(header: &str) -> BTreeSet<String> {
    header
        .lines()
        .filter_map(|line| {
            let line = line.trim();
            let declaration = line
                .strip_prefix("typedef struct ")
                .or_else(|| line.strip_prefix("typedef union "))?;
            let name = declaration
                .split(|c: char| !c.is_ascii_alphanumeric() && c != '_')
                .next()?;
            // Exclude token-pasting templates such as stack_st_##name.
            (c_identifier(name) && !name.ends_with('_')).then(|| name.to_string())
        })
        .collect()
}

fn object_files(dir: &Path, result: &mut Vec<PathBuf>) -> Result<()> {
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if entry.file_type()?.is_dir() {
            object_files(&path, result)?;
        } else if matches!(
            path.extension().and_then(|ext| ext.to_str()),
            Some("o" | "obj")
        ) {
            result.push(path);
        }
    }
    Ok(())
}

fn headers(
    symbols: &BTreeSet<String>,
    data: &BTreeSet<String>,
    tags: &BTreeSet<String>,
) -> (String, String, String) {
    let mut c = String::from("// Generated by scripts/boringssl.rs.\n#pragma once\n");
    for name in symbols.union(tags) {
        if tags.contains(name) || data.contains(name) || name == "p_thread_callback_boringssl" {
            // Some C data is declared without extern "C" by C++ consumers;
            // redefine_extname only affects declarations with C linkage.
            // The TLS callback also occurs in an MSVC /INCLUDE pragma, which
            // BoringSSL expands and stringifies. Preserve that expansion.
            c.push_str(&format!("#define {name} cronet_bssl_{name}\n"));
        } else {
            // Clang supports this on ELF, Mach-O and COFF. Unlike a macro it
            // does not rename unrelated C++ enum members (e.g. crypto::SHA256).
            c.push_str(&format!(
                "#pragma redefine_extname {name} cronet_bssl_{name}\n"
            ));
        }
    }
    let mut asm = String::from("// Generated by scripts/boringssl.rs.\n#if defined(__APPLE__)\n");
    let mut nasm =
        String::from("; Generated by scripts/boringssl.rs.\n%ifidn __OUTPUT_FORMAT__, win32\n");
    for name in symbols {
        asm.push_str(&format!("#define _{name} _cronet_bssl_{name}\n"));
        nasm.push_str(&format!("%xdefine _{name} _cronet_bssl_{name}\n"));
    }
    asm.push_str("#else\n");
    nasm.push_str("%else\n");
    for name in symbols {
        asm.push_str(&format!("#define {name} cronet_bssl_{name}\n"));
        nasm.push_str(&format!("%xdefine {name} cronet_bssl_{name}\n"));
    }
    asm.push_str("#endif\n");
    nasm.push_str("%endif\n");
    (c, asm, nasm)
}

/// Read the target's unprefixed BoringSSL objects, including source-set assembly
/// objects that are not members of libboringssl.a, then generate all three forms.
pub fn generate_headers(src: &Path, bootstrap: &str, outdir: &str, apple: bool) -> Result<()> {
    let mut objects = Vec::new();
    object_files(
        &src.join(bootstrap).join("obj/third_party/boringssl"),
        &mut objects,
    )?;
    objects.sort();
    let mut symbols = BTreeSet::new();
    let mut data = BTreeSet::new();
    // Keep each invocation below Windows' command-line length limit.
    for batch in objects.chunks(20) {
        let text = llvm_nm(src, batch)?;
        symbols.extend(c_symbols(&text, apple, false));
        data.extend(c_symbols(&text, apple, true));
    }
    if !symbols.contains("SSL_new") {
        return Err("bootstrap BoringSSL archive has no SSL_new".into());
    }
    let mut tags = BTreeSet::new();
    for entry in fs::read_dir(src.join("third_party/boringssl/src/include/openssl"))? {
        let path = entry?.path();
        if path.extension().is_some_and(|ext| ext == "h") {
            tags.extend(struct_tags(&fs::read_to_string(path)?));
        }
    }
    if !tags.contains("ssl_st") || !tags.contains("ssl_ctx_st") {
        return Err("BoringSSL opaque SSL struct tags were not found".into());
    }
    let dir = src.join(outdir).join("gen/cronet_boringssl");
    fs::create_dir_all(&dir)?;
    let (c, asm, nasm) = headers(&symbols, &data, &tags);
    write_changed(&dir.join("boringssl_prefix_symbols.h"), &c)?;
    write_changed(&dir.join("boringssl_prefix_symbols_asm.h"), &asm)?;
    write_changed(&dir.join("boringssl_prefix_symbols_nasm.inc"), &nasm)?;
    Ok(())
}

pub fn verify_archive(src: &Path, archive: &Path, apple: bool) -> Result<()> {
    let text = llvm_nm(src, &[archive.to_path_buf()])?;
    let symbols = c_symbols(&text, apple, false);
    if !symbols.contains("Cronet_Engine_Create")
        || !symbols.contains("cronet_bssl_SSL_new")
        || symbols.contains("SSL_new")
        || text.contains("@bssl@@")
        || text.contains("_ZN4bssl")
        || text.contains("_ZNK4bssl")
    {
        return Err(format!(
            "Cronet ABI or BoringSSL isolation check failed: {}",
            archive.display()
        )
        .into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_macho_c_symbols_without_including_cpp_or_archive_headers() {
        assert_eq!(
            c_symbols(
                "ssl.o:\n_SSL_new T 0 10\n__ZN4bssl3FooEv T 20 10\n",
                true,
                false
            ),
            BTreeSet::from(["SSL_new".into()])
        );
        assert_eq!(
            c_symbols(
                "ssl.obj:\nSSL_new T 0 10\n?Foo@bssl@@YAXXZ T 20 10\n",
                false,
                false
            ),
            BTreeSet::from(["SSL_new".into()])
        );
    }

    #[test]
    fn includes_opaque_tags_but_not_comments_or_token_pasting_templates() {
        assert_eq!(struct_tags("// typedef struct wrong Nope;\ntypedef struct ssl_st SSL;\ntypedef union bio_addr_st BIO_ADDR;\ntypedef struct stack_st_##name STACK;"),
            BTreeSet::from(["ssl_st".into(), "bio_addr_st".into()]));
    }

    #[test]
    fn c_headers_do_not_rewrite_unrelated_cpp_identifiers() {
        let symbols = BTreeSet::from([
            "SHA256".into(),
            "p_thread_callback_boringssl".into(),
            "kOpenSSLReasonValues".into(),
        ]);
        let data = c_symbols(
            "SHA256 T 0 10\nkOpenSSLReasonValues R 0 10\np_thread_callback_boringssl D 0 8\n",
            false,
            true,
        );
        assert_eq!(data.len(), 2);
        let (c, asm, nasm) = headers(&symbols, &data, &BTreeSet::from(["ssl_st".into()]));
        assert!(c.contains("#define kOpenSSLReasonValues cronet_bssl_kOpenSSLReasonValues"));
        assert!(c.contains("#pragma redefine_extname SHA256 cronet_bssl_SHA256"));
        assert!(!c.contains("#define SHA256"));
        assert!(c.contains("#define ssl_st cronet_bssl_ssl_st"));
        assert!(c.contains(
            "#define p_thread_callback_boringssl cronet_bssl_p_thread_callback_boringssl"
        ));
        assert!(asm.contains("#define _SHA256 _cronet_bssl_SHA256"));
        assert!(nasm.contains("%xdefine SHA256 cronet_bssl_SHA256"));
    }
}
