use std::process::Stdio;
use tokio::process::Command;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::sync::mpsc;
use crate::installer::config::LabConfig;

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
        "redhat-lsb", "libpng12", "libpng12.i686", "glibc.i686", "libX11.i686",
        // libXss.so.1 - Cadence GUI tools (Virtuoso, etc.) link against the X
        // screen-saver extension; apr-util - pulled in by license/web-service
        // components some Cadence tools ship with; gdb - provides pstack/gstack,
        // which Virtuoso's PerfDiag looks for on PATH at startup.
        "libXScrnSaver", "apr-util", "gdb",
        // tigervnc-server - the software needed for a lab user's VNC session.
        // Installed here machine-wide; who actually gets a session configured
        // is a separate, explicit choice - see User Management / --grant-vnc.
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

    // 4. Create the edausers group (membership is granted per user, not here
    // - see user_mgr::grant_vnc/grant_env). This is infrastructure only: it
    // does NOT by itself grant write access to /opt/cadence or any other EDA
    // tool tree. See CLAUDE.md for what's actually been granted to it so far.
    send_log(&tx, "[STEP 4/6] Ensuring 'edausers' group exists...");
    let _ = run_cmd("groupadd", &["-f", "edausers"], &tx).await;

    // 5. Configure GDM X11 (WaylandEnable=false)
    send_log(&tx, "[STEP 5/6] Ensuring GDM uses X11 for EDA GUI compatibility...");
    let gdm_conf = "/etc/gdm/custom.conf";
    if std::path::Path::new(gdm_conf).exists() {
        let _ = run_cmd("sed", &["-i", "s/^#WaylandEnable=false/WaylandEnable=false/", gdm_conf], &tx).await;
    }

    // 6. Configure Security Limits for EDA tools
    send_log(&tx, "[STEP 6/6] Updating /etc/security/limits.conf for EDA tools...");
    let limits_file = "/etc/security/limits.conf";
    let limits_content = format!(
        "\n# VLSI Lab limits\n{} hard nofile 65536\n{} soft nofile 65536\n{} hard nproc 65536\n{} soft nproc 65536\n",
        student_user, student_user, student_user, student_user
    );
    if let Ok(mut f) = std::fs::OpenOptions::new().append(true).open(limits_file) {
        use std::io::Write;
        let _ = write!(f, "{}", limits_content);
    }

    config.mark_phase_done("PRE_INSTALL").map_err(|e| e.to_string())?;
    send_log(&tx, "[SUCCESS] Phase 0 Pre-installation completed successfully!");
    Ok(())
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
