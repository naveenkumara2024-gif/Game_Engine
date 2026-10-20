//! `cargo xtask <cmd>` — dev automation (lore §6.2 `xtask/`).
//!
//! - `ci`: fmt check → clippy `-D warnings` → tests → check-layers (CLAUDE.md §10).
//! - `check-layers`: fail if any workspace dependency edge breaks CLAUDE.md §5.

use std::process::Command;

use anyhow::{Context, Result, bail};

fn main() -> Result<()> {
    let cmd = std::env::args().nth(1).unwrap_or_default();
    match cmd.as_str() {
        "ci" => ci(),
        "check-layers" => check_layers(),
        _ => {
            eprintln!("usage: cargo xtask <ci | check-layers>");
            std::process::exit(2);
        }
    }
}

fn ci() -> Result<()> {
    cargo(&["fmt", "--all", "--", "--check"])?;
    cargo(&[
        "clippy",
        "--workspace",
        "--all-targets",
        "--",
        "-D",
        "warnings",
    ])?;
    cargo(&["test", "--workspace"])?;
    check_layers()?;
    println!("xtask ci: all checks passed");
    Ok(())
}

fn cargo(args: &[&str]) -> Result<()> {
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
    println!("xtask: cargo {}", args.join(" "));
    let status = Command::new(cargo)
        .args(args)
        .status()
        .context("failed to spawn cargo")?;
    if !status.success() {
        bail!("`cargo {}` failed ({status})", args.join(" "));
    }
    Ok(())
}

fn check_layers() -> Result<()> {
    let metadata = cargo_metadata::MetadataCommand::new().no_deps().exec()?;
    let members: Vec<&str> = metadata
        .workspace_packages()
        .iter()
        .map(|p| p.name.as_str())
        .collect();
    let mut edges = Vec::new();
    let mut external = Vec::new();
    for pkg in metadata.workspace_packages() {
        for dep in &pkg.dependencies {
            if members.contains(&dep.name.as_str()) {
                edges.push((pkg.name.to_string(), dep.name.clone()));
            } else {
                external.push((pkg.name.to_string(), dep.name.clone()));
            }
        }
    }
    let mut violations = layers::check(&edges);
    violations.extend(layers::check_external(&external));
    if violations.is_empty() {
        println!("xtask check-layers: {} workspace edges OK", edges.len());
        return Ok(());
    }
    for v in &violations {
        eprintln!("layering violation: {v}");
    }
    bail!(
        "{} layering violation(s); see CLAUDE.md §5",
        violations.len()
    )
}

mod layers {
    //! A crate may depend only on crates with a strictly lower rank, plus explicit bans.

    /// Rank table mirroring CLAUDE.md §5. Unknown crates fail the check on purpose:
    /// adding a crate means placing it in the layer cake.
    pub fn rank(name: &str) -> Option<u32> {
        Some(match name {
            "ge_core" => 0,
            "ge_alloc" => 1,
            "ge_platform" => 2,
            "ge_tasks" | "ge_rhi" => 3,
            "ge_reflect" | "ge_reflect_derive" => 4,
            "ge_ecs" | "ge_shader" => 5,
            "ge_asset" => 6,
            "ge_render" => 7,
            "ge_physics" | "ge_audio" | "ge_anim" | "ge_ui" | "ge_net" | "ge_script" => 8,
            "ge_engine" => 9,
            n if n == "ge_editor" || n.ends_with("_editor") => 10,
            "sample" => 10,                                        // games/*
            "game" | "editor" | "cooker" | "crash_reporter" => 11, // apps/*
            "xtask" => 12,
            _ => return None,
        })
    }

    /// Edges forbidden even though the ranks would allow them (lore §2.13: UI and lower
    /// layers must build without the renderer and the engine).
    const BANNED: &[(&str, &str)] = &[("ge_ui", "ge_render"), ("ge_ui", "ge_engine")];

    pub fn check(edges: &[(String, String)]) -> Vec<String> {
        let mut out = Vec::new();
        for (from, to) in edges {
            let (Some(rf), Some(rt)) = (rank(from), rank(to)) else {
                let unknown = if rank(from).is_none() { from } else { to };
                out.push(format!(
                    "`{unknown}` has no layer; add it to xtask's rank table"
                ));
                continue;
            };
            let runtime_to_editor = !from.ends_with("editor") && to.ends_with("_editor");
            if rt >= rf || runtime_to_editor {
                out.push(format!(
                    "`{from}` (layer {rf}) must not depend on `{to}` (layer {rt})"
                ));
            } else if BANNED.contains(&(from.as_str(), to.as_str())) {
                out.push(format!("`{from}` must not depend on `{to}` (explicit ban)"));
            }
        }
        out
    }

    /// Third-party crates that only their facade crate may name (CLAUDE.md §4: bevy stays
    /// behind `ge_ecs`, wgpu behind `ge_rhi`; law 12).
    const FACADES: &[(&str, &str)] = &[("bevy_", "ge_ecs"), ("wgpu", "ge_rhi")];

    pub fn check_external(deps: &[(String, String)]) -> Vec<String> {
        deps.iter()
            .filter_map(|(from, to)| {
                let (prefix, owner) = FACADES.iter().find(|(p, _)| to.starts_with(p))?;
                (from != owner).then(|| {
                    format!(
                        "`{from}` must not depend on `{to}`; only `{owner}` may use `{prefix}*`"
                    )
                })
            })
            .collect()
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        fn e(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
            pairs
                .iter()
                .map(|(a, b)| (a.to_string(), b.to_string()))
                .collect()
        }

        #[test]
        fn current_graph_passes() {
            let edges = e(&[
                ("ge_platform", "ge_core"),
                ("ge_rhi", "ge_core"),
                ("ge_engine", "ge_rhi"),
                ("ge_engine", "ge_platform"),
                ("game", "ge_engine"),
            ]);
            assert!(check(&edges).is_empty());
        }

        #[test]
        fn inverted_edge_fails() {
            assert_eq!(check(&e(&[("ge_core", "ge_rhi")])).len(), 1);
            assert_eq!(check(&e(&[("ge_render", "ge_editor")])).len(), 1);
            assert_eq!(check(&e(&[("ge_rhi", "ge_tasks")])).len(), 1, "same layer");
        }

        #[test]
        fn facade_crates_are_exclusive() {
            let ok = e(&[
                ("ge_ecs", "bevy_ecs"),
                ("ge_rhi", "wgpu"),
                ("ge_render", "glam"),
            ]);
            assert!(check_external(&ok).is_empty());
            let bad = e(&[("sample", "bevy_ecs"), ("ge_render", "wgpu")]);
            assert_eq!(check_external(&bad).len(), 2);
        }

        #[test]
        fn bans_and_unknowns_fail() {
            assert_eq!(check(&e(&[("ge_ui", "ge_render")])).len(), 1);
            assert_eq!(check(&e(&[("ge_mystery", "ge_core")])).len(), 1);
        }
    }
}
