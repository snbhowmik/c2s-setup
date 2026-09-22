use std::collections::BTreeSet;
use std::fs;
use std::path::Path;
use std::process::Stdio;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command;
use tokio::sync::mpsc;

#[derive(Debug, Clone)]
pub struct LabUser {
    pub username: String,
    pub role: String, // "Student", "Faculty", "Staff", "Guest"
    pub identifier: String, // Reg No or FET ID
    pub exists: bool,
    pub bashrc_configured: bool,
    pub vnc_configured: bool,
}

pub fn check_user_exists(username: &str) -> bool {
    std::process::Command::new("id")
        .arg(username)
        .output()
        .map(|out| out.status.success())
        .unwrap_or(false)
}

pub fn is_bashrc_configured(username: &str) -> bool {
    let bashrc_path = format!("/home/{}/.bashrc", username);
    if !Path::new(&bashrc_path).exists() {
        return false;
    }
    fs::read_to_string(&bashrc_path)
        .map(|content| content.contains("/opt/vlsilab/eda-launcher.sh"))
        .unwrap_or(false)
}

/// True if /etc/tigervnc/vncserver.users maps any display to this user.
pub fn is_vnc_configured(username: &str) -> bool {
    let content = fs::read_to_string("/etc/tigervnc/vncserver.users").unwrap_or_default();
    content.lines().any(|line| {
        let line = line.trim();
        !line.is_empty() && !line.starts_with('#')
            && line.split_once('=').map(|(_, u)| u.trim() == username).unwrap_or(false)
    })
}

/// Real Linux accounts in the "lab user" uid range (1000-59999) - one source
/// of truth for "which users exist on this machine", shared by the Dashboard
/// (sys_validation.rs) and the User Management screen, so the two never show
/// a different answer to the same question.
pub fn list_lab_users() -> Vec<LabUser> {
    let mut users = Vec::new();
    if let Ok(out) = std::process::Command::new("sh")
        .args(&["-c", "getent passwd | awk -F: '$3 >= 1000 && $3 < 60000 {print $1}'"])
        .output()
    {
        let text = String::from_utf8_lossy(&out.stdout);
        for uname in text.lines() {
            let uname = uname.trim();
            if uname.is_empty() {
                continue;
            }
            users.push(LabUser {
                username: uname.to_string(),
                role: infer_role(uname),
                identifier: "-".to_string(),
                exists: true,
                bashrc_configured: is_bashrc_configured(uname),
                vnc_configured: is_vnc_configured(uname),
            });
        }
    }
    users.sort_by(|a, b| a.username.cmp(&b.username));
    users
}

/// Best-effort guess for display purposes only - membership in `wheel` (sudo
/// access) or a `sysadmin`-prefixed name reads as Sysadmin, everything else
/// as Student. Nothing in this codebase grants privileges based on this.
fn infer_role(username: &str) -> String {
    let in_wheel = std::process::Command::new("id")
        .args(&["-nG", username])
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).split_whitespace().any(|g| g == "wheel"))
        .unwrap_or(false);
    if in_wheel || username.starts_with("sysadmin") {
        "Sysadmin".to_string()
    } else {
        "Student".to_string()
    }
}

pub async fn create_or_configure_student_user(
    username: &str,
    role: &str,
    identifier: &str,
    tx: mpsc::UnboundedSender<String>,
) -> Result<(), String> {
    send_log(&tx, &format!("[USER MGR] Provisioning user account '{}' ({}) - ID: {}", username, role, identifier));

    if !check_user_exists(username) {
        send_log(&tx, &format!("[USER MGR] User '{}' does not exist. Creating system account...", username));
        let output = std::process::Command::new("useradd")
            .args(&["-m", "-s", "/bin/bash", "-c", &format!("VLSI Lab {} [{}]", role, identifier), username])
            .output();

        match output {
            Ok(out) if out.status.success() => {
                send_log(&tx, &format!("[USER MGR] Successfully created Linux user '{}'.", username));
            }
            Ok(out) => {
                let err_msg = String::from_utf8_lossy(&out.stderr);
                send_log(&tx, &format!("[ERROR] useradd failed: {}", err_msg));
                return Err(format!("useradd failed: {}", err_msg));
            }
            Err(e) => return Err(format!("Failed to execute useradd: {}", e)),
        }
    } else {
        send_log(&tx, &format!("[USER MGR] User account '{}' already exists.", username));
    }

    grant_env(username, &tx).await
}

/// Injects EDA launcher sourcing into an *existing* user's .bashrc if it
/// isn't there already - this is the "env" grant. Does not create the user;
/// see create_or_configure_student_user for that (which calls this too).
pub async fn grant_env(username: &str, tx: &mpsc::UnboundedSender<String>) -> Result<(), String> {
    let bashrc_path = format!("/home/{}/.bashrc", username);
    if !Path::new(&bashrc_path).exists() {
        send_log(tx, &format!("[WARN] Home directory or .bashrc missing for '{}'.", username));
        return Err(format!("{} does not exist", bashrc_path));
    }
    if is_bashrc_configured(username) {
        send_log(tx, &format!("[INFO] .bashrc for '{}' is already configured.", username));
        return Ok(());
    }
    send_log(tx, &format!("[USER MGR] Injecting EDA launcher into {}...", bashrc_path));
    let entry = "\n# Source VLSI Lab EDA Tool Launcher\nif [ -f /opt/vlsilab/eda-launcher.sh ]; then\n    source /opt/vlsilab/eda-launcher.sh\nfi\n";
    let existing = fs::read_to_string(&bashrc_path).map_err(|e| e.to_string())?;
    fs::write(&bashrc_path, format!("{}{}", existing, entry)).map_err(|e| format!("Failed to update {}: {}", bashrc_path, e))?;
    send_log(tx, &format!("[SUCCESS] Configured .bashrc for '{}'.", username));
    Ok(())
}

// ─────────────────────────────────────────────────────────────────────────────
// VNC granting - opt-in, per user, chosen by the sysadmin (not auto-applied
// during pre-install to whichever user LabConfig happens to name).
// ─────────────────────────────────────────────────────────────────────────────

/// Grants `username` a persistent VNC session: allocates (or reuses) a
/// display, adds them to `edausers`, sets a fresh VNC auth password (returned
/// so the caller can surface it - this is the only time it's ever visible
/// again), and enables+starts `vncserver@<display>.service`. Reachable only
/// over an SSH tunnel per the lab's access pattern (`ssh -L
/// 590N:localhost:590N user@host`, then a VNC viewer at `localhost:590N`) -
/// this never listens beyond localhost itself.
///
/// Uses tigervnc-server's actual vncsession/systemd framework on RHEL8
/// (1.13+): /etc/tigervnc/vncserver.users maps a display to a username, and
/// `vncserver-config-defaults` (shipped by the package, left untouched here)
/// already defaults new sessions to a GNOME desktop.
pub async fn grant_vnc(username: &str, tx: &mpsc::UnboundedSender<String>) -> Result<String, String> {
    if !check_user_exists(username) {
        return Err(format!("user '{}' does not exist", username));
    }

    let display = allocate_vnc_display(username);
    send_log(tx, &format!("[USER MGR] Granting VNC (display {}) to '{}'...", display, username));

    ensure_edausers_member(username, tx).await?;
    ensure_vnc_user_mapping(&display, username, tx).await?;
    let password = set_vnc_password(username, tx).await?;

    let unit = format!("vncserver@{}.service", display);
    run_cmd("systemctl", &["daemon-reload"], tx).await?;
    run_cmd("systemctl", &["enable", "--now", &unit], tx).await?;

    let port = 5900 + display.trim_start_matches(':').parse::<u32>().unwrap_or(1);
    send_log(tx, &format!(
        "[SUCCESS] VNC granted to '{}': password {}  (write this down now - it is not saved anywhere else and won't be shown again). Reach it with: ssh -L {}:localhost:{} {}@<this-host>, then point a VNC viewer at localhost:{}.",
        username, password, port, port, username, port
    ));
    Ok(password)
}

async fn ensure_edausers_member(username: &str, tx: &mpsc::UnboundedSender<String>) -> Result<(), String> {
    let _ = run_cmd("groupadd", &["-f", "edausers"], tx).await;
    run_cmd("usermod", &["-aG", "edausers", username], tx).await
}

/// Finds `username`'s existing VNC display in vncserver.users, or allocates
/// the lowest unused one (starting at :1) if they don't have one yet.
fn allocate_vnc_display(username: &str) -> String {
    let content = fs::read_to_string("/etc/tigervnc/vncserver.users").unwrap_or_default();
    let mut used = BTreeSet::new();
    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some((display, user)) = line.split_once('=') {
            let display = display.trim();
            let user = user.trim();
            if user == username {
                return display.to_string();
            }
            if let Some(n) = display.strip_prefix(':').and_then(|n| n.parse::<u32>().ok()) {
                used.insert(n);
            }
        }
    }
    let mut n = 1;
    while used.contains(&n) {
        n += 1;
    }
    format!(":{}", n)
}

/// Appends `<display>=<username>` to /etc/tigervnc/vncserver.users, replacing
/// any prior mapping for that display rather than duplicating it - idempotent
/// across re-runs.
async fn ensure_vnc_user_mapping(display: &str, username: &str, tx: &mpsc::UnboundedSender<String>) -> Result<(), String> {
    let path = "/etc/tigervnc/vncserver.users";
    let target_line = format!("{}={}", display, username);
    let prefix = format!("{}=", display);

    let existing = tokio::fs::read_to_string(path).await.unwrap_or_default();
    if existing.lines().any(|l| l.trim() == target_line) {
        send_log(tx, &format!("[INFO] VNC display {} already mapped to '{}'.", display, username));
        return Ok(());
    }

    let mut kept: Vec<&str> = existing.lines().filter(|l| !l.trim_start().starts_with(&prefix)).collect();
    kept.push(&target_line);
    let new_content = format!("{}\n", kept.join("\n"));

    tokio::fs::write(path, new_content).await.map_err(|e| format!("failed to write {}: {}", path, e))?;
    send_log(tx, &format!("[SUCCESS] Mapped VNC display {} to '{}' in {}.", display, username, path));
    Ok(())
}

/// Generates a fresh VNC password and sets it via `vncpasswd -f` (the
/// documented non-interactive form: reads one line from stdin, writes the
/// obfuscated password to stdout, never touches a file itself), then writes
/// the result into <home>/.vnc/passwd with the ownership/permissions
/// TigerVNC expects, fixing SELinux context afterward per tigervnc's own
/// HOWTO.md. Returns the plaintext password so the caller can surface it once.
async fn set_vnc_password(username: &str, tx: &mpsc::UnboundedSender<String>) -> Result<String, String> {
    let password = random_password(8);

    let mut child = Command::new("vncpasswd")
        .arg("-f")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("failed to spawn vncpasswd: {}", e))?;

    {
        let mut stdin = child.stdin.take().ok_or("vncpasswd gave no stdin")?;
        stdin.write_all(password.as_bytes()).await.map_err(|e| e.to_string())?;
        stdin.write_all(b"\n").await.map_err(|e| e.to_string())?;
    }

    let output = child.wait_with_output().await.map_err(|e| e.to_string())?;
    if !output.status.success() {
        return Err(format!(
            "vncpasswd exited with {:?}: {}",
            output.status.code(),
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }

    let vnc_dir = Path::new("/home").join(username).join(".vnc");
    tokio::fs::create_dir_all(&vnc_dir).await.map_err(|e| format!("failed to create {}: {}", vnc_dir.display(), e))?;
    let passwd_path = vnc_dir.join("passwd");
    tokio::fs::write(&passwd_path, &output.stdout).await.map_err(|e| format!("failed to write {}: {}", passwd_path.display(), e))?;

    let owner = format!("{}:{}", username, username);
    let vnc_dir_str = vnc_dir.to_string_lossy().to_string();
    let passwd_path_str = passwd_path.to_string_lossy().to_string();
    run_cmd("chown", &["-R", &owner, &vnc_dir_str], tx).await?;
    run_cmd("chmod", &["700", &vnc_dir_str], tx).await?;
    run_cmd("chmod", &["600", &passwd_path_str], tx).await?;
    // Not every system runs SELinux in enforcing mode, and restorecon may not
    // be installed at all - non-fatal either way.
    let _ = run_cmd("restorecon", &["-RFv", &vnc_dir_str], tx).await;

    Ok(password)
}

/// len random characters from an unambiguous alphabet (no 0/O, 1/l/I) read
/// from /dev/urandom - a human has to type this into a VNC client once.
fn random_password(len: usize) -> String {
    const CHARSET: &[u8] = b"ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz23456789";
    let mut buf = vec![0u8; len];
    let read_ok = fs::File::open("/dev/urandom")
        .and_then(|mut f| { use std::io::Read; f.read_exact(&mut buf) })
        .is_ok();
    if !read_ok {
        // /dev/urandom is present on every Linux system this installer
        // targets; this is only a last-resort so the caller never panics.
        use std::time::{SystemTime, UNIX_EPOCH};
        let seed = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.subsec_nanos()).unwrap_or(0);
        for (i, b) in buf.iter_mut().enumerate() {
            *b = ((seed as usize).wrapping_add(i.wrapping_mul(7919))) as u8;
        }
    }
    buf.iter().map(|b| CHARSET[*b as usize % CHARSET.len()] as char).collect()
}

fn send_log(tx: &mpsc::UnboundedSender<String>, msg: &str) {
    tx.send(msg.to_string()).ok();
}

async fn run_cmd(cmd: &str, args: &[&str], tx: &mpsc::UnboundedSender<String>) -> Result<(), String> {
    send_log(tx, &format!("$ {} {}", cmd, args.join(" ")));

    let mut child = Command::new(cmd)
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("Failed to spawn {}: {}", cmd, e))?;

    let stdout = child.stdout.take().unwrap();
    let stderr = child.stderr.take().unwrap();

    let tx_out = tx.clone();
    let mut reader_out = BufReader::new(stdout).lines();
    tokio::spawn(async move {
        while let Ok(Some(line)) = reader_out.next_line().await {
            tx_out.send(line).ok();
        }
    });

    let tx_err = tx.clone();
    let mut reader_err = BufReader::new(stderr).lines();
    tokio::spawn(async move {
        while let Ok(Some(line)) = reader_err.next_line().await {
            tx_err.send(format!("[STDERR] {}", line)).ok();
        }
    });

    let status = child.wait().await.map_err(|e| e.to_string())?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("Command '{}' failed with code {:?}", cmd, status.code()))
    }
}
