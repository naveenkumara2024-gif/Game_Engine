//! Shader library (lore §6.7 "Shaders"): compiles `shaders/*.wesl` to WGSL with the `wesl`
//! crate (imports + `@if` permutations) and, with the `hot-reload` feature, watches the folder
//! so the renderer can rebuild pipelines while the game runs.
//!
//! This crate produces **WGSL text only**; turning it into GPU objects is `ge_rhi`'s job, so the
//! cooker (M10) can reuse it without a device. Reading source shaders at runtime is a dev-only
//! convenience until shaders are cooked (law 7).

#![forbid(unsafe_code)]

#[cfg(feature = "hot-reload")]
mod watch;

use std::path::{Path, PathBuf};

/// A compiled permutation, ready for `Rhi::create_shader_module`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CompiledShader {
    /// Module name relative to the shader root, without extension (`"mesh"`).
    pub name: String,
    pub wgsl: String,
}

#[derive(Debug, thiserror::Error)]
pub enum ShaderError {
    #[error("shader '{name}' failed to compile:\n{message}")]
    Compile { name: String, message: String },
    #[error("shader watcher: {0}")]
    Watch(String),
}

/// Source shaders under one root folder. `package::` imports resolve against that root.
#[derive(Debug)]
pub struct ShaderLibrary {
    root: PathBuf,
    #[cfg(feature = "hot-reload")]
    watcher: Option<watch::ShaderWatcher>,
}

impl ShaderLibrary {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self {
            root: root.into(),
            #[cfg(feature = "hot-reload")]
            watcher: None,
        }
    }

    /// `GE_SHADER_DIR` if set, else the workspace `shaders/` folder baked in at build time
    /// (dev builds run from the source tree; packaged builds will load cooked shaders).
    pub fn default_root() -> PathBuf {
        std::env::var_os("GE_SHADER_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| Path::new(env!("CARGO_MANIFEST_DIR")).join("../../shaders"))
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Compiles `<root>/<name>.wesl`. `features` switch `@if(name)` blocks; any flag not listed
    /// is treated as off, so every permutation is explicit.
    pub fn compile(
        &self,
        name: &str,
        features: &[(&str, bool)],
    ) -> Result<CompiledShader, ShaderError> {
        let _span = tracing::info_span!("Shader::Compile", name).entered();
        let mut options = wesl::CompileOptions::default();
        options.features.default = wesl::Feature::Disable;
        for &(flag, on) in features {
            options.features.set(flag, on);
        }
        let compiler = wesl::Compiler::new_with_resolver(
            options,
            wesl::resolver::FileResolver::new(&self.root),
        );
        let path = self.root.join(format!("{name}.wesl"));
        if !path.is_file() {
            return Err(ShaderError::Compile {
                name: name.to_owned(),
                message: format!("{} not found", path.display()),
            });
        }
        // Build the module path ourselves: wesl 0.6's `compile(fs_path)` turns a single-segment
        // relative path into a package name and resolves `package.wesl` instead.
        let module = wesl::syntax::ModulePath::new(
            wesl::syntax::PathOrigin::Absolute,
            name.split('/').map(str::to_owned).collect(),
        );
        compiler
            .compile_module(&module)
            .map(|result| CompiledShader {
                name: name.to_owned(),
                wgsl: result.to_string(),
            })
            .map_err(|e| ShaderError::Compile {
                name: name.to_owned(),
                message: e.to_string(),
            })
    }

    /// Starts watching the root folder for `.wesl`/`.wgsl` edits. A no-op without the
    /// `hot-reload` feature.
    pub fn watch(&mut self) -> Result<(), ShaderError> {
        #[cfg(feature = "hot-reload")]
        {
            let watcher = watch::ShaderWatcher::new(&self.root)
                .map_err(|e| ShaderError::Watch(e.to_string()))?;
            tracing::info!(root = %self.root.display(), "shader hot reload watching");
            self.watcher = Some(watcher);
        }
        Ok(())
    }

    /// True once per burst of shader edits (debounced), meaning "recompile now". Call once per
    /// frame on the main thread.
    pub fn poll_changed(&mut self) -> bool {
        #[cfg(feature = "hot-reload")]
        if let Some(w) = &mut self.watcher {
            return w.poll(std::time::Instant::now());
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A unique scratch shader folder per test.
    fn scratch(name: &str, files: &[(&str, &str)]) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("ge_shader_{name}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        for (path, src) in files {
            let p = dir.join(path);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, src).unwrap();
        }
        dir
    }

    const COMMON: &str = "fn helper() -> f32 { return 2.0; }\n";
    const MAIN: &str = "\
import package::common::util::helper;

@if(fancy) fn tint() -> f32 { return helper() * 10.0; }
@if(!fancy) fn tint() -> f32 { return helper(); }

@fragment fn fs_main() -> @location(0) vec4<f32> { return vec4(tint()); }
";

    #[test]
    fn imports_and_permutations() {
        let dir = scratch("perm", &[("common/util.wesl", COMMON), ("main.wesl", MAIN)]);
        let lib = ShaderLibrary::new(&dir);

        let plain = lib.compile("main", &[]).unwrap();
        assert!(plain.wgsl.contains("fs_main"), "{}", plain.wgsl);
        assert!(plain.wgsl.contains("2.0"), "import inlined: {}", plain.wgsl);
        assert!(
            !plain.wgsl.contains("10.0"),
            "fancy off by default: {}",
            plain.wgsl
        );
        assert!(!plain.wgsl.contains("import"), "{}", plain.wgsl);

        let fancy = lib.compile("main", &[("fancy", true)]).unwrap();
        assert!(fancy.wgsl.contains("10.0"), "{}", fancy.wgsl);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn errors_are_reported_not_panicked() {
        let dir = scratch("err", &[("bad.wesl", "fn broken( { }")]);
        let lib = ShaderLibrary::new(&dir);
        assert!(matches!(
            lib.compile("bad", &[]),
            Err(ShaderError::Compile { .. })
        ));
        assert!(matches!(
            lib.compile("missing", &[]),
            Err(ShaderError::Compile { .. })
        ));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn engine_shaders_compile() {
        let lib = ShaderLibrary::new(ShaderLibrary::default_root());
        for debug_normals in [false, true] {
            let out = lib
                .compile("mesh", &[("debug_normals", debug_normals)])
                .unwrap_or_else(|e| panic!("{e}"));
            assert!(out.wgsl.contains("vs_main") && out.wgsl.contains("fs_main"));
        }
    }
}
