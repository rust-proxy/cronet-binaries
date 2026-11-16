//! Select the CRT independently of Chromium's component/shared-library build.

use std::{error::Error, fs, path::Path};

fn patched(source: &str) -> Result<String, Box<dyn Error>> {
    if source.contains("cronet_dynamic_crt = false") {
        return Ok(source.to_owned());
    }
    let mut result = source.to_owned();
    let declaration =
        "declare_args() {\n  # Turn this on to have the linker output extra timing information.";
    if result.matches(declaration).count() != 1 {
        return Err("Windows CRT GN declaration anchor changed".into());
    }
    result = result.replace(declaration, "declare_args() {\n  # Keep a complete static Cronet archive while using the dynamic CRT.\n  cronet_dynamic_crt = false\n\n  # Turn this on to have the linker output extra timing information.");
    for config in ["default_crt", "release_crt"] {
        let old = format!("config(\"{config}\") {{\n  if (is_component_build) {{");
        if result.matches(&old).count() != 1 {
            return Err(format!("Windows CRT GN anchor changed: {config}").into());
        }
        result = result.replace(&old, &format!("config(\"{config}\") {{\n  if (is_component_build ||\n      (cronet_dynamic_crt && current_toolchain == default_toolchain)) {{"));
    }
    Ok(result)
}

pub fn patch_gn(src: &Path) -> Result<(), Box<dyn Error>> {
    let path = src.join("build/config/win/BUILD.gn");
    let source = fs::read_to_string(&path)?.replace("\r\n", "\n");
    let result = patched(&source)?;
    if result != source {
        fs::write(path, result)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn overrides_both_crt_configs_without_enabling_components() {
        let source = "declare_args() {\n  # Turn this on to have the linker output extra timing information.\n}\nconfig(\"default_crt\") {\n  if (is_component_build) {\n  }\n}\nconfig(\"release_crt\") {\n  if (is_component_build) {\n  }\n}";
        let result = patched(source).unwrap();
        assert_eq!(
            result
                .matches("current_toolchain == default_toolchain")
                .count(),
            2
        );
        assert!(result.contains("cronet_dynamic_crt = false"));
        assert!(!result.contains("is_component_build = true"));
        assert_eq!(patched(&result).unwrap(), result);
        assert!(patched(&source.replace("release_crt", "renamed_crt")).is_err());
    }
}
