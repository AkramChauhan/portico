use crate::paths;
use std::os::unix::fs::PermissionsExt;
use std::process::Command;

/// Run a shell script as root behind the standard macOS authentication dialog.
///
/// The script is written into a user-owned 0700 directory rather than /tmp:
/// it is executed as root, so a world-writable location would let any local
/// process rewrite it between creation and execution.
pub fn run_as_root(script: &str) -> anyhow::Result<String> {
    let dir = paths::root().join("tmp");
    std::fs::create_dir_all(&dir)?;
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700))?;

    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let path = dir.join(format!("setup-{}-{}.sh", std::process::id(), stamp));

    std::fs::write(&path, script)?;
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700))?;

    let escaped = path
        .to_string_lossy()
        .replace('\\', "\\\\")
        .replace('"', "\\\"");
    let applescript = format!(
        "do shell script \"/bin/sh -e \\\"{escaped}\\\" 2>&1\" with administrator privileges"
    );

    let out = Command::new("/usr/bin/osascript")
        .arg("-e")
        .arg(&applescript)
        .output();

    let _ = std::fs::remove_file(&path);

    let out = out?;
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        if err.contains("-128") {
            anyhow::bail!("Cancelled at the password prompt");
        }
        anyhow::bail!("Privileged step failed: {}", err.trim());
    }
    Ok(stdout)
}
