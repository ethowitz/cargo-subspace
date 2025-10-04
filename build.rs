use std::env;
use std::process::Command;

use anyhow::{Result, anyhow};

fn main() -> Result<()> {
    let output = Command::new("git").arg("rev-parse").arg("HEAD").output()?;

    if output.status.success() {
        let profile = env::var("PROFILE").unwrap_or_else(|_| "unknown profile".into());
        let hash = String::from_utf8(output.stdout)?;

        println!("cargo::rustc-env=GIT_COMMIT_HASH={hash}");
        println!("cargo::rustc-env=CARGO_BUILD_PROFILE={profile}");

        Ok(())
    } else {
        Err(anyhow!("Failed to get commit hash"))
    }
}
