//! Keeps the app's own rendering off the NVIDIA dGPU.
//!
//! GTK's default renderer enumerates every Vulkan/EGL device at startup, which opens `/dev/nvidia*`: the
//! app would then show up in `GpuHolders`, wake the RTX 4070 and block the Éco profile from removing it.
//! The binary re-executes itself once with these variables (setting them in-process is `unsafe` in Rust
//! 2024, and the crate forbids `unsafe`). Variables already set by the user are left alone.

use std::path::Path;

/// Marker set on the re-executed process (prevents loops).
pub const MARKER: &str = "MSI_CENTER_GPU_ENV";

/// Mesa's EGL vendor file (Intel iGPU).
pub const MESA_EGL_VENDOR: &str = "/usr/share/glvnd/egl_vendor.d/50_mesa.json";

/// Variables to add, given a lookup of the current environment and whether the Mesa EGL vendor file
/// exists. Empty when nothing is needed (already applied, or everything set by the user).
#[must_use]
pub fn wanted(get: impl Fn(&str) -> Option<String>, mesa_vendor: bool) -> Vec<(&'static str, String)> {
    if get(MARKER).is_some() {
        return Vec::new();
    }
    let mut out = Vec::new();
    if get("VK_LOADER_DRIVERS_DISABLE").is_none() {
        // Vulkan loader ≥ 1.3.234: never load the NVIDIA ICD.
        out.push(("VK_LOADER_DRIVERS_DISABLE", "*nvidia*".to_owned()));
    }
    if mesa_vendor && get("__EGL_VENDOR_LIBRARY_FILENAMES").is_none() {
        out.push(("__EGL_VENDOR_LIBRARY_FILENAMES", MESA_EGL_VENDOR.to_owned()));
    }
    if !out.is_empty() {
        out.push((MARKER, "1".to_owned()));
    }
    out
}

/// [`wanted`] for the real process environment.
#[must_use]
pub fn wanted_now() -> Vec<(&'static str, String)> {
    wanted(|k| std::env::var(k).ok(), Path::new(MESA_EGL_VENDOR).exists())
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;

    fn env(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let m: HashMap<String, String> = pairs
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect();
        move |k| m.get(k).cloned()
    }

    #[test]
    fn adds_the_variables_once() {
        let w = wanted(env(&[]), true);
        assert_eq!(
            w,
            vec![
                ("VK_LOADER_DRIVERS_DISABLE", "*nvidia*".to_owned()),
                ("__EGL_VENDOR_LIBRARY_FILENAMES", MESA_EGL_VENDOR.to_owned()),
                (MARKER, "1".to_owned()),
            ]
        );
        assert_eq!(wanted(env(&[(MARKER, "1")]), true), vec![]);
    }

    #[test]
    fn respects_user_choices_and_missing_mesa() {
        let w = wanted(env(&[("VK_LOADER_DRIVERS_DISABLE", "")]), false);
        assert_eq!(w, vec![]);
        let w = wanted(env(&[]), false);
        assert_eq!(w.len(), 2);
    }
}
