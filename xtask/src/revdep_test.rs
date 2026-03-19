use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use clap::Args;
use xshell::{cmd, Shell};

/// Pinned reverse dependencies for reproducible testing.
/// Update these periodically — prefer tags over bare commit hashes
/// so that pins are easy to audit and understand at a glance.
const REVDEPS: &[RevDep] = &[
    RevDep {
        name: "cargo",
        repo: "https://github.com/rust-lang/cargo.git",
        // cargo 0.94 ships with Rust 1.93
        rev: "0.94.0",
        toolchain: None,
    },
    RevDep {
        name: "cargo-vendor-filterer",
        repo: "https://github.com/coreos/cargo-vendor-filterer.git",
        rev: "v0.5.18",
        toolchain: None,
    },
    RevDep {
        name: "crates-io",
        repo: "https://github.com/rust-lang/crates.io.git",
        // crates.io doesn't tag releases; pin to a commit.
        rev: "e482ed3da791735f37489cb9e6410a3b768d51f1",
        // crates.io pins a specific Rust version via rust-toolchain.toml
        // that may not be installed; override to stable.
        toolchain: Some("stable"),
    },
];

struct RevDep {
    name: &'static str,
    repo: &'static str,
    /// Git revision to check out — may be a tag name or a commit hash.
    rev: &'static str,
    /// If set, override RUSTUP_TOOLCHAIN for this revdep (e.g. when the
    /// project has a rust-toolchain.toml pinning a version we don't have).
    toolchain: Option<&'static str>,
}

#[derive(Args)]
pub(crate) struct RevdepTestArgs {
    /// Specific revdeps to test (default: all). Available: cargo,
    /// cargo-vendor-filterer, crates-io
    targets: Vec<String>,

    /// Self-test mode: inject a compile error into Builder::new and
    /// verify that at least one revdep test fails. Validates that the
    /// CI pipeline actually catches tar-rs regressions.
    #[arg(long)]
    self_test: bool,
}

pub(crate) fn run(args: RevdepTestArgs) -> Result<()> {
    let tar_rs_root = project_root()?;
    if args.self_test {
        run_self_test(&tar_rs_root)
    } else {
        let revdeps = resolve_targets(&args.targets)?;
        run_revdep_tests(&tar_rs_root, &revdeps)
    }
}

fn project_root() -> Result<PathBuf> {
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR")
        .context("CARGO_MANIFEST_DIR not set — run via `cargo xtask`")?;
    // xtask is at <root>/xtask, so go up one level
    let root = Path::new(&manifest_dir)
        .parent()
        .context("could not find project root")?
        .to_owned();
    Ok(root)
}

fn resolve_targets(targets: &[String]) -> Result<Vec<&'static RevDep>> {
    if targets.is_empty() {
        return Ok(REVDEPS.iter().collect());
    }
    targets
        .iter()
        .map(|name| {
            REVDEPS
                .iter()
                .find(|r| r.name == name.as_str())
                .with_context(|| {
                    let available: Vec<_> = REVDEPS.iter().map(|r| r.name).collect();
                    format!(
                        "unknown revdep {name:?}, available: {}",
                        available.join(", ")
                    )
                })
        })
        .collect()
}

fn patch_config_flag(tar_rs_root: &Path) -> String {
    let path = tar_rs_root.display();
    format!("patch.crates-io.tar.path=\"{path}\"")
}

/// Clone a repo at a specific revision (tag or commit hash).
fn clone_at_rev(sh: &Shell, repo: &str, rev: &str, dest: &Path) -> Result<()> {
    // `rev^{commit}` dereferences tags to their underlying commit, but
    // xshell's cmd! macro doesn't allow literal braces in the template.
    // Build the rev-parse argument as a plain string instead.
    let rev_deref = format!("{rev}^{{commit}}");

    if dest.join(".git").is_dir() {
        // Resolve what the requested rev points to, so we can compare
        // against HEAD regardless of whether rev is a tag or a hash.
        let wanted = cmd!(sh, "git -C {dest} rev-parse {rev_deref}")
            .ignore_status()
            .read()?;
        let current = cmd!(sh, "git -C {dest} rev-parse HEAD").read()?;
        if current.trim() == wanted.trim() && !wanted.is_empty() {
            println!(":: {}: already at {rev}", dest.display());
            return Ok(());
        }
        println!(
            ":: {}: wrong rev ({}), re-fetching...",
            dest.display(),
            current.trim()
        );
        cmd!(sh, "git -C {dest} fetch origin {rev}").run()?;
        cmd!(sh, "git -C {dest} checkout {rev}").run()?;
    } else {
        println!(":: Cloning {repo} at {rev}...");
        cmd!(sh, "git clone --no-checkout {repo} {dest}").run()?;
        cmd!(sh, "git -C {dest} checkout {rev}").run()?;
    }
    Ok(())
}

/// Build a `cargo` command with the `--config patch` flag and optional
/// toolchain override applied.
fn cargo_cmd<'a>(sh: &'a Shell, tar_rs_root: &Path, revdep: &RevDep) -> xshell::Cmd<'a> {
    let patch = patch_config_flag(tar_rs_root);
    let c = cmd!(sh, "cargo --config {patch}");
    if let Some(tc) = revdep.toolchain {
        c.env("RUSTUP_TOOLCHAIN", tc)
    } else {
        c
    }
}

/// Update the downstream lockfile so it picks up our local `tar` version.
///
/// Without this, `--config patch.crates-io.tar.path=...` is silently ignored
/// when the downstream `Cargo.lock` pins a different `tar` version than the
/// one in our working tree (e.g. lockfile has 0.4.44 but we're at 0.4.45).
/// Running `cargo update -p tar` with the patch config applied forces the
/// lockfile to resolve to the patched version.
fn update_tar_in_lockfile(sh: &Shell, tar_rs_root: &Path, revdep: &RevDep) -> Result<()> {
    println!(":: Updating lockfile to use local tar-rs...");
    cargo_cmd(sh, tar_rs_root, revdep)
        .args(["update", "-p", "tar"])
        .run()
        .context("failed to update tar in downstream lockfile")?;
    Ok(())
}

fn test_cargo(sh: &Shell, tar_rs_root: &Path, revdep: &RevDep) -> Result<()> {
    let dest = tar_rs_root.join("target/revdeps/cargo");
    clone_at_rev(sh, revdep.repo, revdep.rev, &dest)?;
    let _dir = sh.push_dir(&dest);
    update_tar_in_lockfile(sh, tar_rs_root, revdep)?;

    println!(":: Building cargo workspace...");
    cargo_cmd(sh, tar_rs_root, revdep)
        .args(["check", "--workspace"])
        .run()?;

    println!(":: Running cargo package tests (exercises tar read+write paths)...");
    // The package:: integration tests exercise tar::Builder (creating .crate
    // files) and tar::Archive (unpacking them for validation).
    //
    // The filter "package::" is a substring match, so it also hits tests from
    // other modules that happen to contain "package" in their path (e.g.
    // cargo_add::manifest_path_package, cargo_info::*_package, etc.).
    // Those are cargo-internal snapshot tests unrelated to tar, and they
    // break when the installed Rust version differs from what the pinned
    // cargo version expects. Skip them explicitly.
    cargo_cmd(sh, tar_rs_root, revdep)
        .args([
            "test",
            "-p",
            "cargo",
            "--test",
            "testsuite",
            "--",
            "package::",
            "--skip",
            "cargo_add::",
            "--skip",
            "cargo_info::",
            "--skip",
            "cargo_package::",
            "--skip",
            "cargo_remove::",
        ])
        .run()?;

    Ok(())
}

fn test_cargo_vendor_filterer(sh: &Shell, tar_rs_root: &Path, revdep: &RevDep) -> Result<()> {
    let dest = tar_rs_root.join("target/revdeps/cargo-vendor-filterer");
    clone_at_rev(sh, revdep.repo, revdep.rev, &dest)?;
    let _dir = sh.push_dir(&dest);
    update_tar_in_lockfile(sh, tar_rs_root, revdep)?;

    println!(":: Running cargo-vendor-filterer tests (exercises tar write path)...");
    // Skip tar_zstd which requires the external zstd CLI.
    cargo_cmd(sh, tar_rs_root, revdep)
        .args(["test", "--", "--skip", "tar_zstd"])
        .run()?;

    Ok(())
}

fn test_crates_io(sh: &Shell, tar_rs_root: &Path, revdep: &RevDep) -> Result<()> {
    let dest = tar_rs_root.join("target/revdeps/crates-io");
    clone_at_rev(sh, revdep.repo, revdep.rev, &dest)?;
    let _dir = sh.push_dir(&dest);
    update_tar_in_lockfile(sh, tar_rs_root, revdep)?;

    println!(":: Running crates_io_tarball tests (exercises tar Builder round-trip)...");
    // Tests cover Header::new_gnu, append_data, into_inner, size limits,
    // symlink/hardlink rejection — no database needed.
    cargo_cmd(sh, tar_rs_root, revdep)
        .args(["test", "-p", "crates_io_tarball"])
        .run()?;

    Ok(())
}

fn run_test_for_revdep(sh: &Shell, tar_rs_root: &Path, revdep: &RevDep) -> Result<()> {
    println!();
    println!("========================================");
    println!("  Testing reverse dep: {}", revdep.name);
    println!("========================================");
    println!();

    match revdep.name {
        "cargo" => test_cargo(sh, tar_rs_root, revdep),
        "cargo-vendor-filterer" => test_cargo_vendor_filterer(sh, tar_rs_root, revdep),
        "crates-io" => test_crates_io(sh, tar_rs_root, revdep),
        other => bail!("no test function for revdep {other:?}"),
    }
}

fn run_revdep_tests(tar_rs_root: &Path, revdeps: &[&RevDep]) -> Result<()> {
    let sh = Shell::new()?;
    let revdep_dir = tar_rs_root.join("target/revdeps");
    sh.create_dir(&revdep_dir)?;

    let mut failed = Vec::new();
    for revdep in revdeps {
        if let Err(e) = run_test_for_revdep(&sh, tar_rs_root, revdep) {
            println!("  FAILED: {}: {e:#}", revdep.name);
            failed.push(revdep.name);
        }
    }

    println!();
    println!("========================================");
    println!("  Reverse dependency testing summary");
    println!("========================================");
    if failed.is_empty() {
        println!("  All reverse dependency tests passed.");
        Ok(())
    } else {
        bail!("revdep tests failed: {}", failed.join(", "));
    }
}

/// Self-test: inject a compile error into Builder::new, verify that a revdep
/// test fails, then restore. This validates that the CI pipeline actually
/// catches tar-rs regressions rather than passing vacuously.
fn run_self_test(tar_rs_root: &Path) -> Result<()> {
    let sh = Shell::new()?;
    let builder_rs = tar_rs_root.join("src/builder.rs");
    let revdep_dir = tar_rs_root.join("target/revdeps");
    sh.create_dir(&revdep_dir)?;

    println!();
    println!("========================================");
    println!("  Self-test: verifying the pipeline");
    println!("  detects tar-rs regressions");
    println!("========================================");
    println!();

    // Inject compile error into Builder::new
    println!(":: Injecting compile_error! into Builder::new...");
    let original = sh.read_file(&builder_rs)?;
    let broken = original.replace(
        "pub fn new(obj: W) -> Builder<W> {",
        "pub fn new(obj: W) -> Builder<W> { compile_error!(\"revdep self-test: intentional breakage\");",
    );
    if broken == original {
        bail!("failed to inject compile_error — Builder::new signature not found");
    }
    sh.write_file(&builder_rs, &broken)?;

    // Use cargo-vendor-filterer as the fast canary (small, quick to build)
    let cvf = REVDEPS
        .iter()
        .find(|r| r.name == "cargo-vendor-filterer")
        .unwrap();

    println!(":: Running cargo-vendor-filterer (expecting failure)...");
    let result = run_test_for_revdep(&sh, tar_rs_root, cvf);

    // Restore original file
    println!(":: Restoring builder.rs...");
    sh.write_file(&builder_rs, &original)?;

    match result {
        Ok(()) => {
            bail!(
                "self-test FAILED: revdep tests passed despite injected compile error!\n\
                 The CI pipeline is not actually exercising tar-rs code paths."
            );
        }
        Err(_) => {
            println!();
            println!(":: Good — revdep tests failed as expected.");
            println!(":: Self-test passed: pipeline correctly detects tar-rs breakage.");
            Ok(())
        }
    }
}
