mod revdep_test;

use anyhow::Result;
use clap::Parser;

#[derive(Parser)]
#[command(about = "tar-rs development tasks")]
enum Cli {
    /// Run reverse dependency tests.
    ///
    /// Clones known reverse dependencies at pinned revisions, patches them
    /// to use our local tar checkout via `cargo --config`, and runs their
    /// test suites. This catches regressions that our own tests might miss.
    RevdepTest(revdep_test::RevdepTestArgs),
}

fn main() -> Result<()> {
    match Cli::parse() {
        Cli::RevdepTest(args) => revdep_test::run(args),
    }
}
