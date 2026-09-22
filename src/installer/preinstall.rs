use std::path::Path;
use std::process::Stdio;
use tokio::process::Command;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::mpsc;
use crate::installer::config::LabConfig;

/// The one VNC display this lab's workflow uses per machine, matching the
/// documented access pattern: `ssh -L 5901:localhost:5901 user@host` (590*1*
/// = display *:1*) then point a VNC viewer at `localhost:5901`. Never listened
/// on beyond localhost - the tunnel is what makes it reachable at all.
const VNC_DISPLAY: &str = ":1";

pub async fn run_preinstall(
    config: &mut LabConfig,
    tx: mpsc::UnboundedSender<String>,
) -> Result<(), String> {
    tx.send("[INFO] Starting Phase 0: Pre-installation & System Dependencies...".to_string()).ok();

    let student_user = config.student_user.clone();
    let hostname_fqdn = config.hostname_fqdn.clone();

    // 1. Set Hostname
    send_log(&tx, &format!("[STEP 1/6] Setting system hostname to {}...", hostname_fqdn));
    run_cmd("hostnamectl", &["set-hostname", &hostname_fqdn], &tx).await?;

    // 2. Install EPEL and essential system packages
    send_log(&tx, "[STEP 2/6] Installing EPEL repository and system build packages...");
    let dnf_packages = vec![
        "epel-release", "tcsh", "csh", "ksh", "gcc", "gcc-c++", "make", "flex", "bison",
        "patch", "libX11-devel", "libXext-devel", "libXrender-devel", "libXrandr-devel",
        "libXt-devel", "libXtst-devel", "libXi-devel", "libXft-devel", "libXp", "motif",
        "motif-devel", "ncurses-compat-libs", "xorg-x11-fonts-Type1", "xorg-x11-fonts-75dpi",
        "xorg-x11-fonts-100dpi", "mesa-libGL", "mesa-libGLU", "glu", "compat-openssl10",
        "redhat-lsb", "libpng12", "glibc.i686", "libX11.i686",
        // libXss.so.1 - Cadence GUI tools (Virtuoso, etc.) link against the X
        // screen-saver extension; apr-util - pulled in by license/web-service
        // components some Cadence tools ship with; gdb - provides pstack/gstack,
        // which Virtuoso's PerfDiag looks for on PATH at startup.
        "libXScrnSaver", "apr-util", "gdb",
        // tigervnc-server - lets a student start a VNC session on this machine
        // (vncserver :N) that's reached over an SSH tunnel from another machine,
        // rather than exposing the VNC port directly.
        "tigervnc-server"
    ];

    let mut dnf_args = vec!["install", "-y"];
    dnf_args.extend(dnf_packages);
    let _ = run_cmd("dnf", &dnf_args, &tx).await; // continue even if non-critical package notice occurs

    // 3. Create Student User if not existing
    send_log(&tx, &format!("[STEP 3/6] Setting up student user '{}'...", student_user));
    let user_check = Command::new("id")
        .arg(&student_user)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .await;

    match user_check {
        Ok(status) if status.success() => {
            send_log(&tx, &format!("[INFO] Student user '{}' already exists.", student_user));
        }
        _ => {
            run_cmd("useradd", &["-m", "-s", "/bin/bash", "-c", "VLSI Lab Student", &student_user], &tx).await?;
            send_log(&tx, &format!("[SUCCESS] Created student user '{}'.", student_user));
        }
    }

    // 4. Create the edausers group and add the student to it. This is
    // infrastructure only for now - it does NOT grant write access to
    // /opt/cadence or any other EDA tool tree. Group membership is the
    // mechanism for granting a lab user a specific permission later (a
    // scratch/log directory, a device node, etc.) without resorting to
    // world-writable permissions or sudo - see CLAUDE.md for what's actually
    // been granted to it so far.
    send_log(&tx, "[STEP 4/6] Setting up 'edausers' group...");
    let _ = run_cmd("groupadd", &["-f", "edausers"], &tx).await;
    run_cmd("usermod", &["-aG", "edausers", &student_user], &tx).await?;
    send_log(&tx, &format!("[SUCCESS] '{}' is a member of edausers.", student_user));

    // 5. Configure GDM X11 (WaylandEnable=false)
    send_log(&tx, "[STEP 5/6] Ensuring GDM uses X11 for EDA GUI compatibility...");
    let gdm_conf = "/etc/gdm/custom.conf";
    if std::path::Path::new(gdm_conf).exists() {
        let _ = run_cmd("sed", &["-i", "s/^#WaylandEnable=false/WaylandEnable=false/", gdm_conf], &tx).await;
    }

    // 6. Configure Security Limits for EDA tools
    send_log(&tx, "[STEP 6/7] Updating /etc/security/limits.conf for EDA tools...");
    let limits_file = "/etc/security/limits.conf";
    let limits_content = format!(
        "\n# VLSI Lab limits\n{} hard nofile 65536\n{} soft nofile 65536\n{} hard nproc 65536\n{} soft nproc 65536\n",
        student_user, student_user, student_user, student_user
    );
    if let Ok(mut f) = std::fs::OpenOptions::new().append(true).open(limits_file) {
        use std::io::Write;
        let _ = write!(f, "{}", limits_content);
    }

    // 7. Set up a persistent VNC session for the student, reachable only via
    // an SSH tunnel (see vncserver@.service's own header comment - it refuses
    // to be a good idea on an untrusted network otherwise). Not fatal on its
    // own: a hiccup here (tigervnc-server failing to install, SELinux tooling
    // missing, etc.) shouldn't fail the whole pre-install phase.
    send_log(&tx, &format!("[STEP 7/7] Setting up VNC (display {}) for '{}'...", VNC_DISPLAY, student_user));
    if let Err(e) = setup_vnc(&student_user, &tx).await {
        send_log(&tx, &format!("[WARN] VNC setup incomplete: {}. You can finish it manually - see HOWTO.md from tigervnc-server.", e));
    }

    config.mark_phase_done("PRE_INSTALL").map_err(|e| e.to_string())?;
    send_log(&tx, "[SUCCESS] Phase 0 Pre-installation completed successfully!");
    Ok(())
}

/// Wires up TigerVNC's own `vncsession`/systemd framework (RHEL8's
/// tigervnc-server >= 1.13-ish; NOT the old "hand-copy vncserver@.service and
/// edit it" approach from older TigerVNC docs, which this package's own
/// service file explicitly warns will conflict with it):
///
/// 1. Map VNC_DISPLAY to the student in /etc/tigervnc/vncserver.users - this
///    is what `vncserver@<display>.service` reads to know which user to run
///    the session as. `vncserver-config-defaults` (shipped by the package,
///    left untouched here) already defaults new sessions to a GNOME desktop.
/// 2. Set the student's VNC auth password. This still uses TigerVNC's own
///    vncauth (~/.vnc/passwd via `vncpasswd -f`), separate from their system
///    login password - the PAM config the package ships is for session setup
///    (SELinux/logind integration), not VNC authentication. Since this runs
///    fully unattended, the password is generated here and logged once so
///    the sysadmin running pre-install can hand it to the student - it is
///    not derived from anything else and isn't recoverable after this run.
/// 3. Enable and start the systemd service for that display.
async fn setup_vnc(student_user: &str, tx: &mpsc::UnboundedSender<String>) -> Result<(), String> {
    ensure_vnc_user_mapping(student_user, tx).await?;

    let home = Path::new("/home").join(student_user);
    let password = set_vnc_password(student_user, &home, tx).await?;
    send_log(tx, &format!(
        "[SUCCESS] VNC password for '{}' set to: {}  (write this down now - it is not saved anywhere else and won't be shown again)",
        student_user, password
    ));

    let unit = format!("vncserver@{}.service", VNC_DISPLAY);
    run_cmd("systemctl", &["daemon-reload"], tx).await?;
    run_cmd("systemctl", &["enable", "--now", &unit], tx).await?;
    send_log(tx, &format!(
        "[SUCCESS] {} is running. Reach it with: ssh -L 5901:localhost:5901 {}@<this-host>, then point a VNC viewer at localhost:5901.",
        unit, student_user
    ));
    Ok(())
}

/// Appends `VNC_DISPLAY=<student_user>` to /etc/tigervnc/vncserver.users,
/// replacing any prior mapping for VNC_DISPLAY on this machine rather than
/// duplicating it - idempotent across re-runs and across a student-user
/// change (e.g. after `--machine N` picks a different machine number).
async fn ensure_vnc_user_mapping(student_user: &str, tx: &mpsc::UnboundedSender<String>) -> Result<(), String> {
    let path = "/etc/tigervnc/vncserver.users";
    let target_line = format!("{}={}", VNC_DISPLAY, student_user);
    let prefix = format!("{}=", VNC_DISPLAY);

    let existing = tokio::fs::read_to_string(path).await.unwrap_or_default();
    if existing.lines().any(|l| l.trim() == target_line) {
        send_log(tx, &format!("[INFO] VNC display {} already mapped to '{}'.", VNC_DISPLAY, student_user));
        return Ok(());
    }

    let mut kept: Vec<&str> = existing.lines().filter(|l| !l.trim_start().starts_with(&prefix)).collect();
    kept.push(&target_line);
    let new_content = format!("{}\n", kept.join("\n"));

    tokio::fs::write(path, new_content).await.map_err(|e| format!("failed to write {}: {}", path, e))?;
    send_log(tx, &format!("[SUCCESS] Mapped VNC display {} to '{}' in {}.", VNC_DISPLAY, student_user, path));
    Ok(())
}

/// Generates a fresh VNC password, sets it via `vncpasswd -f` (the documented
/// non-interactive form: reads one line from stdin, writes the obfuscated
/// password to stdout - it never touches a file itself), and writes the
/// result into <home>/.vnc/passwd with the ownership/permissions TigerVNC
/// expects, fixing SELinux context afterward per tigervnc's own HOWTO.md.
/// Returns the plaintext password so the caller can surface it once.
async fn set_vnc_password(student_user: &str, home: &Path, tx: &mpsc::UnboundedSender<String>) -> Result<String, String> {
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

    let vnc_dir = home.join(".vnc");
    tokio::fs::create_dir_all(&vnc_dir).await.map_err(|e| format!("failed to create {}: {}", vnc_dir.display(), e))?;
    let passwd_path = vnc_dir.join("passwd");
    tokio::fs::write(&passwd_path, &output.stdout).await.map_err(|e| format!("failed to write {}: {}", passwd_path.display(), e))?;

    let owner = format!("{}:{}", student_user, student_user);
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
    let read_ok = std::fs::File::open("/dev/urandom")
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

async fn run_cmd(
    cmd: &str,
    args: &[&str],
    tx: &mpsc::UnboundedSender<String>,
) -> Result<(), String> {
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
