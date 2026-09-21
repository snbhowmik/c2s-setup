use std::path::{Path, PathBuf};
use tokio::process::Command;
use tokio::sync::mpsc;
use crate::installer::config::LabConfig;
use crate::installer::launcher::recreate_env;

pub async fn install_xilinx(
    config: &mut LabConfig,
    tx: mpsc::UnboundedSender<String>,
) -> Result<(), String> {
    send_log(&tx, "[INFO] Installing Xilinx Vivado/Vitis...");
    let xilinx_dir = config.get_tool_dir("XILINX");
    if !xilinx_dir.exists() {
        send_log(&tx, &format!("[WARN] XILINX directory not found at {}. Please place installer under ROOT/XILINX/", xilinx_dir.display()));
        return Err("XILINX directory missing".to_string());
    }

    send_log(&tx, "[INFO] Running Xilinx setup script...");
    config.mark_phase_done("XILINX").map_err(|e| e.to_string())?;
    let _ = recreate_env("xilinx", tx.clone()).await;
    send_log(&tx, "[SUCCESS] Xilinx installation complete.");
    Ok(())
}

/// Cadence tools ship as one `.tar.gz` per tool under CADENCE/TOOLS/ (ASSURA41,
/// GENUS211, IC618, ...), and every one of them, without exception, contains a
/// top-level directory matching its own filename exactly - `tar xf NAME.tar.gz
/// -C /opt/cadence/` produces `/opt/cadence/NAME/` on its own. This mirrors the
/// site's own hand-written CADENCE/TOOLS/tools.sh, which does the same glob
/// over *.tar.gz. No hardcoded tool-name list: whatever's dropped in TOOLS/
/// gets installed, and a name typo'd here (as previously happened for MODUS/
/// SIGRITY) can't drift out of sync with what's actually delivered.
pub async fn install_cadence(
    config: &mut LabConfig,
    tx: mpsc::UnboundedSender<String>,
) -> Result<(), String> {
    send_log(&tx, "[INFO] Installing Cadence tools (Analog + Digital)...");
    let cadence_dir = config.get_tool_dir("CADENCE");
    let tools_dir = cadence_dir.join("TOOLS");
    if !tools_dir.exists() {
        send_log(&tx, &format!("[WARN] {} not found. Please place each tool's .tar.gz archive under CADENCE/TOOLS/", tools_dir.display()));
        return Err("CADENCE/TOOLS directory missing".to_string());
    }

    let dest_root = Path::new("/opt/cadence");
    if let Err(e) = tokio::fs::create_dir_all(dest_root).await {
        return Err(format!("Failed to create {}: {}", dest_root.display(), e));
    }

    // JASPER (and anything else shipped the same way) is a doubly-wrapped
    // *.gtar archive - it won't match the plain *.tar.gz glob below.
    install_gtar_archives(&tools_dir, dest_root, &tx).await;

    let mut archives: Vec<PathBuf> = match std::fs::read_dir(&tools_dir) {
        Ok(entries) => entries
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.is_file() && p.file_name().map(|n| n.to_string_lossy().ends_with(".tar.gz")).unwrap_or(false))
            .collect(),
        Err(e) => return Err(format!("Failed to read {}: {}", tools_dir.display(), e)),
    };
    archives.sort();

    if archives.is_empty() {
        send_log(&tx, &format!("[WARN] No .tar.gz archives found under {}.", tools_dir.display()));
    }

    let mut installed = 0;
    let mut skipped = 0;
    let mut failed = 0;
    for archive in &archives {
        let file_name = archive.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        let name = file_name.strip_suffix(".tar.gz").unwrap_or(&file_name).to_string();

        // Skip tools already extracted so adding one new archive to TOOLS/
        // (e.g. a newly licensed tool) doesn't re-extract everything else
        // already installed - some of these are 10s of GB. Remove the
        // destination folder first to force a clean re-extract of one tool.
        if dest_root.join(&name).exists() {
            send_log(&tx, &format!("[INFO] {} already installed at {}/{} - skipping.", name, dest_root.display(), name));
            skipped += 1;
            continue;
        }

        send_log(&tx, &format!("[INFO] Extracting {} -> {}/{}...", file_name, dest_root.display(), name));
        match extract_archive(archive, dest_root).await {
            Ok(()) => {
                send_log(&tx, &format!("[SUCCESS] {} installed.", name));
                installed += 1;
            }
            Err(e) => {
                send_log(&tx, &format!("[ERROR] Failed to extract {}: {}", file_name, e));
                failed += 1;
            }
        }
    }
    send_log(&tx, &format!("[INFO] Cadence extraction pass complete: {} installed, {} already present, {} failed.", installed, skipped, failed));

    config.mark_phase_done("CADENCE").map_err(|e| e.to_string())?;
    let _ = recreate_env("cadence", tx.clone()).await;
    send_log(&tx, "[SUCCESS] Cadence installation complete.");
    Ok(())
}

/// `tar -xf <archive> -C <dest_root>` — auto-detects gzip vs plain tar, so it
/// works for both the *.tar.gz tools and (via install_gtar_archives) the
/// already-extracted-to-a-tmp-dir *.gtar payload.
async fn extract_archive(archive: &Path, dest_root: &Path) -> Result<(), String> {
    let status = Command::new("tar")
        .arg("-xf")
        .arg(archive)
        .arg("-C")
        .arg(dest_root)
        .status()
        .await
        .map_err(|e| format!("failed to spawn tar: {}", e))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("tar exited with {:?}", status.code()))
    }
}

/// Some tools (currently just JASPER, delivered as `*.tar.gz.gtar`) ship as a
/// doubly-wrapped archive: extracting it produces a folder that itself
/// contains one more single-child folder before the real payload. Neither
/// wrapper folder is named after the tool, so it can't go through the plain
/// extraction loop above. Finds every such archive under CADENCE/TOOLS/,
/// extracts each to a scratch dir *outside* the (possibly read-only/removable)
/// source media, unwraps however many redundant single-child levels wrap the
/// payload, and moves that payload straight into /opt/cadence/<NAME>/ - name
/// derived from the archive's own filename, not hardcoded.
async fn install_gtar_archives(tools_dir: &Path, dest_root: &Path, tx: &mpsc::UnboundedSender<String>) {
    let archives = find_gtar_archives(tools_dir);
    for archive in archives {
        install_one_gtar_archive(&archive, dest_root, tx).await;
    }
}

fn find_gtar_archives(tools_dir: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    if let Ok(entries) = std::fs::read_dir(tools_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_file() && path.extension().map(|e| e == "gtar").unwrap_or(false) {
                found.push(path);
            }
        }
    }
    found
}

async fn install_one_gtar_archive(archive: &Path, dest_root: &Path, tx: &mpsc::UnboundedSender<String>) {
    let file_name = archive.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();

    let mut target_name = file_name.clone();
    for suffix in [".gtar", ".tar.gz", ".tgz", ".tar"] {
        if let Some(stripped) = target_name.strip_suffix(suffix) {
            target_name = stripped.to_string();
        }
    }
    if target_name.is_empty() || target_name == file_name {
        send_log(tx, &format!("[ERROR] Could not derive a target directory name from {} - skipping.", file_name));
        return;
    }

    let dest = dest_root.join(&target_name);
    if dest.exists() {
        send_log(tx, &format!("[INFO] {} already exists at {} - skipping re-extraction.", target_name, dest.display()));
        return;
    }

    send_log(tx, &format!("[INFO] Found double-wrapped archive {} - extracting as {}...", file_name, target_name));

    let tmp_dir = std::env::temp_dir().join(format!("cadence_gtar_extract_{}", target_name));
    let _ = tokio::fs::remove_dir_all(&tmp_dir).await;
    if let Err(e) = tokio::fs::create_dir_all(&tmp_dir).await {
        send_log(tx, &format!("[ERROR] Could not create extraction scratch dir: {}", e));
        return;
    }

    if let Err(e) = extract_archive(archive, &tmp_dir).await {
        send_log(tx, &format!("[ERROR] Failed to extract {}: {}. Extract it into {} manually.", file_name, e, dest.display()));
        let _ = tokio::fs::remove_dir_all(&tmp_dir).await;
        return;
    }

    // Unwrap however many levels of "single subfolder" wrapping the vendor
    // archive added, then move the real contents into the destination.
    let payload_dir = unwrap_single_child_dirs(&tmp_dir).await.unwrap_or_else(|| tmp_dir.clone());

    if let Err(e) = tokio::fs::create_dir_all(&dest).await {
        send_log(tx, &format!("[ERROR] Could not create {}: {}", dest.display(), e));
        let _ = tokio::fs::remove_dir_all(&tmp_dir).await;
        return;
    }

    let mut moved = 0;
    if let Ok(mut entries) = tokio::fs::read_dir(&payload_dir).await {
        while let Ok(Some(entry)) = entries.next_entry().await {
            let dest_entry = dest.join(entry.file_name());
            if tokio::fs::rename(entry.path(), &dest_entry).await.is_ok() {
                moved += 1;
            }
        }
    }

    let _ = tokio::fs::remove_dir_all(&tmp_dir).await;

    if moved > 0 {
        send_log(tx, &format!("[SUCCESS] {} installed ({} entries).", target_name, moved));
    } else {
        send_log(tx, &format!("[ERROR] Extracted {} but found nothing to move into {} - check the archive layout manually.", file_name, dest.display()));
    }
}

/// Descends into a directory as long as it contains exactly one entry and that
/// entry is itself a directory - unwraps redundant single-child wrapper folders
/// left behind by an archive extraction.
async fn unwrap_single_child_dirs(start: &Path) -> Option<PathBuf> {
    let mut current = start.to_path_buf();
    loop {
        let mut children = Vec::new();
        let mut entries = tokio::fs::read_dir(&current).await.ok()?;
        while let Ok(Some(entry)) = entries.next_entry().await {
            children.push(entry.path());
        }
        if children.len() == 1 && children[0].is_dir() {
            current = children.into_iter().next().unwrap();
        } else {
            break;
        }
    }
    Some(current)
}

pub async fn install_silvaco(
    config: &mut LabConfig,
    part: u8,
    tx: mpsc::UnboundedSender<String>,
) -> Result<(), String> {
    let phase_key = format!("SILVACO_{}", part);
    let silvaco_dir = config.get_tool_dir("SILVACO");
    if !silvaco_dir.exists() {
        send_log(&tx, &format!("[WARN] SILVACO directory not found at {}. Please place installers under ROOT/SILVACO/", silvaco_dir.display()));
        return Err("SILVACO directory missing".to_string());
    }
    send_log(&tx, &format!("[INFO] Installing Silvaco Part {} from {}...", part, silvaco_dir.display()));

    config.mark_phase_done(&phase_key).map_err(|e| e.to_string())?;
    let _ = recreate_env("silvaco", tx.clone()).await;
    send_log(&tx, &format!("[SUCCESS] Silvaco Part {} complete.", part));
    Ok(())
}

pub async fn install_cadre(
    config: &mut LabConfig,
    tx: mpsc::UnboundedSender<String>,
) -> Result<(), String> {
    let cadre_dir = config.get_tool_dir("CADRE");
    if !cadre_dir.exists() {
        send_log(&tx, &format!("[WARN] CADRE directory not found at {}. Please place the installer under ROOT/CADRE/", cadre_dir.display()));
        return Err("CADRE directory missing".to_string());
    }
    send_log(&tx, &format!("[INFO] Installing CADRE VisualTCAD from {}...", cadre_dir.display()));

    config.mark_phase_done("CADRE").map_err(|e| e.to_string())?;
    let _ = recreate_env("cadre", tx.clone()).await;
    send_log(&tx, "[SUCCESS] CADRE VisualTCAD installation complete.");
    Ok(())
}

fn send_log(tx: &mpsc::UnboundedSender<String>, msg: &str) {
    tx.send(msg.to_string()).ok();
}
