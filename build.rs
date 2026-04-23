/// Build script: injects the version date and git hash at compile time.
fn main() {
    // Date-based version: YYYY.M.D
    let now = chrono_lite_date();
    println!("cargo:rustc-env=ZOONI_VERSION={now}");

    // Git short hash (if available)
    if let Ok(output) = std::process::Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .output()
    {
        if output.status.success() {
            let hash = String::from_utf8_lossy(&output.stdout).trim().to_string();
            println!("cargo:rustc-env=ZOONI_GIT_HASH={hash}");
        }
    }

    println!("cargo:rerun-if-changed=.git/HEAD");
    println!("cargo:rerun-if-changed=.git/refs/heads/");
}

/// Minimal date without pulling in chrono at build time.
fn chrono_lite_date() -> String {
    // Use the `date` command (works on macOS/Linux)
    if let Ok(output) = std::process::Command::new("date")
        .args(["+%Y.%-m.%-d"])
        .output()
    {
        if output.status.success() {
            return String::from_utf8_lossy(&output.stdout).trim().to_string();
        }
    }
    // Fallback: use Cargo.toml version
    env!("CARGO_PKG_VERSION").to_string()
}
