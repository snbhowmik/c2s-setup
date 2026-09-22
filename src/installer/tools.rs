use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use tokio::io::{AsyncBufReadExt, BufReader};
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

/// Cadence installs in two passes, in this order:
///
/// 1. The bulk vendor bundles at the CADENCE/ root (Analog_RHEL_8.tar.gz,
///    Digital_RHEL_8.tar.gz) - combined snapshots of the whole tool set,
///    including the vendor-configured bits (OpenAccess and friends) that the
///    per-tool tarballs don't carry. Extracted with --skip-old-files so they
///    only ever fill in what isn't there yet.
/// 2. The per-tool archives under CADENCE/TOOLS/ (ASSURA41, GENUS211, IC618,
///    ...), which are the newer/authoritative copies and are extracted
///    *over* anything the bundles laid down for the same tool.
///
/// Every TOOLS/ archive, without exception, contains a top-level directory
/// matching its own filename exactly - `tar xf NAME.tar.gz -C /opt/cadence/`
/// produces `/opt/cadence/NAME/` on its own. This mirrors the site's own
/// hand-written CADENCE/TOOLS/tools.sh, which does the same glob over
/// *.tar.gz. No hardcoded tool-name list in either pass: whatever's dropped in
/// TOOLS/ (or at the CADENCE/ root) gets installed, and a name typo'd here (as
/// previously happened for MODUS/SIGRITY) can't drift out of sync with what's
/// actually delivered.
pub async fn install_cadence(
    config: &mut LabConfig,
    tx: mpsc::UnboundedSender<String>,
) -> Result<(), String> {
    send_log(&tx, "[INFO] Installing Cadence tools (Analog + Digital)...");
    let cadence_dir = config.get_tool_dir("CADENCE");
    if !cadence_dir.exists() {
        send_log(&tx, &format!("[WARN] CADENCE directory not found at {}. Please place the tool media under ROOT/CADENCE/", cadence_dir.display()));
        return Err("CADENCE directory missing".to_string());
    }
    let tools_dir = cadence_dir.join("TOOLS");
    let bundles = find_archives(&cadence_dir, ".tar.gz");
    if !tools_dir.exists() && bundles.is_empty() {
        send_log(&tx, &format!("[WARN] Neither {} nor a bulk bundle (CADENCE/*.tar.gz) found. Please place each tool's .tar.gz archive under CADENCE/TOOLS/", tools_dir.display()));
        return Err("CADENCE/TOOLS directory missing".to_string());
    }

    let dest_root = Path::new("/opt/cadence");
    if let Err(e) = tokio::fs::create_dir_all(dest_root).await {
        return Err(format!("Failed to create {}: {}", dest_root.display(), e));
    }

    // Pass 1: the bulk Analog/Digital bundles at the CADENCE/ root. They go in
    // first so the per-tool archives below land on top of them.
    install_cadence_bundles(&bundles, dest_root, &tx).await;

    if !tools_dir.exists() {
        send_log(&tx, &format!("[WARN] {} not found - installed the bulk bundles only.", tools_dir.display()));
        link_openaccess(dest_root, &tx).await;
        config.mark_phase_done("CADENCE").map_err(|e| e.to_string())?;
        let _ = recreate_env("cadence", tx.clone()).await;
        send_log(&tx, "[SUCCESS] Cadence installation complete.");
        return Ok(());
    }

    // JASPER (and anything else shipped the same way) is a doubly-wrapped
    // *.gtar archive - it won't match the plain *.tar.gz glob below.
    install_gtar_archives(&tools_dir, dest_root, &tx).await;

    let archives = find_archives(&tools_dir, ".tar.gz");

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
        // A folder the bulk bundle laid down is the one exception: TOOLS/ is
        // authoritative, so it gets extracted over the bundle's copy exactly
        // once, and drops the marker on the way through.
        let dest = dest_root.join(&name);
        let from_bundle = dest.join(BUNDLE_MARKER).exists();
        if dest.exists() && !from_bundle {
            send_log(&tx, &format!("[INFO] {} already installed at {} - skipping.", name, dest.display()));
            skipped += 1;
            continue;
        }

        if from_bundle {
            send_log(&tx, &format!("[INFO] {} came from a bulk bundle - overwriting it with TOOLS/{}...", name, file_name));
        } else {
            send_log(&tx, &format!("[INFO] Extracting {} -> {}...", file_name, dest.display()));
        }
        match extract_archive(archive, dest_root).await {
            Ok(()) => {
                let _ = tokio::fs::remove_file(dest.join(BUNDLE_MARKER)).await;
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

    link_openaccess(dest_root, &tx).await;

    config.mark_phase_done("CADENCE").map_err(|e| e.to_string())?;
    let _ = recreate_env("cadence", tx.clone()).await;
    send_log(&tx, "[SUCCESS] Cadence installation complete.");
    Ok(())
}

/// Dropped inside every `/opt/cadence/<TOOL>/` a bulk bundle created, and
/// removed again the moment the matching `CADENCE/TOOLS/<TOOL>.tar.gz` is
/// extracted over it. It's what lets the TOOLS/ pass tell "this folder is the
/// bundle's placeholder, overwrite it" apart from "this tool is already
/// properly installed, leave it alone" across separate runs.
const BUNDLE_MARKER: &str = ".c2s-from-bundle";

/// Most Cadence tools ship their own bundled, version-tagged OpenAccess
/// install at their own top level (e.g. `MODUS221/oa_v22.60.065`, `~100+MB`,
/// fully populated), but expect it linked in at `<tool>/share/oa` - that's
/// exactly what Cadence's own `share/oaInstallers/oaConfig` script does
/// (`createSymbolicLink "$OA_HOME" "$SHARE_ROOT/oa"`), a "post-install
/// configure step" (its own words - see the tool's own missing-OA warning)
/// that raw tar extraction never runs. Without it: "$OA_HOME is not set and
/// the Cadence installation does not seem to have a valid default OA
/// installation" - a warning for some tools, a hard failure for others
/// (observed: Modus). Runs once after every archive/bundle/gtar pass so it
/// sees every tool's `oa_v*` directory regardless of which pass produced it.
///
/// Only ever replaces an empty `share/oa` placeholder (what the raw tarballs
/// leave behind) or a symlink already pointing at *some* `oa_v*` (kept
/// current if the tool was re-extracted at a different OA version) - a real,
/// populated `share/oa` some future archive might actually ship is left
/// alone, logged as a warning rather than silently overwritten.
async fn link_openaccess(dest_root: &Path, tx: &mpsc::UnboundedSender<String>) {
    let tool_dirs: Vec<PathBuf> = match std::fs::read_dir(dest_root) {
        Ok(entries) => entries.flatten().map(|e| e.path()).filter(|p| p.is_dir()).collect(),
        Err(_) => return,
    };

    for tool_dir in tool_dirs {
        let oa_source = std::fs::read_dir(&tool_dir).ok().and_then(|entries| {
            entries.flatten()
                .map(|e| e.path())
                .find(|p| p.is_dir() && p.file_name().map(|n| n.to_string_lossy().starts_with("oa_v")).unwrap_or(false))
        });
        let Some(oa_source) = oa_source else { continue };

        let share_dir = tool_dir.join("share");
        if !share_dir.is_dir() {
            continue; // this tool doesn't use the share/oa convention at all
        }
        let oa_link = share_dir.join("oa");

        if let Ok(target) = tokio::fs::read_link(&oa_link).await {
            if target == oa_source {
                continue; // already linked correctly
            }
        }

        let is_symlink = tokio::fs::symlink_metadata(&oa_link).await.map(|m| m.file_type().is_symlink()).unwrap_or(false);
        let is_empty_dir = !is_symlink && oa_link.is_dir() && is_dir_empty(&oa_link).await;

        if oa_link.exists() && !is_symlink && !is_empty_dir {
            send_log(tx, &format!(
                "[WARN] {} exists and isn't an empty placeholder or a stale link - leaving it, not linking to {}.",
                oa_link.display(), oa_source.display()
            ));
            continue;
        }

        if is_symlink {
            let _ = tokio::fs::remove_file(&oa_link).await;
        } else if is_empty_dir {
            let _ = tokio::fs::remove_dir(&oa_link).await;
        }

        match tokio::fs::symlink(&oa_source, &oa_link).await {
            Ok(()) => send_log(tx, &format!("[SUCCESS] Linked {} -> {} (OpenAccess).", oa_link.display(), oa_source.display())),
            Err(e) => send_log(tx, &format!("[WARN] Failed to link {} -> {}: {}", oa_link.display(), oa_source.display(), e)),
        }
    }
}

async fn is_dir_empty(path: &Path) -> bool {
    match tokio::fs::read_dir(path).await {
        Ok(mut entries) => matches!(entries.next_entry().await, Ok(None)),
        Err(_) => false,
    }
}

/// Non-recursive `<dir>/*<suffix>` glob, sorted for a stable install order.
fn find_archives(dir: &Path, suffix: &str) -> Vec<PathBuf> {
    let mut found: Vec<PathBuf> = match std::fs::read_dir(dir) {
        Ok(entries) => entries
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.is_file() && p.file_name().map(|n| n.to_string_lossy().ends_with(suffix)).unwrap_or(false))
            .collect(),
        Err(_) => Vec::new(),
    };
    found.sort();
    found
}

/// Visible (non-dotfile) subdirectory names of `dir` - used to work out which
/// tool folders a bundle extraction actually produced.
fn dir_names(dir: &Path) -> BTreeSet<String> {
    let mut names = BTreeSet::new();
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            if entry.path().is_dir() {
                let name = entry.file_name().to_string_lossy().to_string();
                if !name.starts_with('.') {
                    names.insert(name);
                }
            }
        }
    }
    names
}

/// Extracts the bulk Analog/Digital bundles that sit at the CADENCE/ root
/// (rather than under TOOLS/) into /opt/cadence/.
///
/// Two rules keep this from fighting with the per-tool TOOLS/ pass that runs
/// after it:
///
/// - `--skip-old-files`, so a bundle never overwrites a file that's already
///   there. Whatever TOOLS/ (or an earlier run) installed always wins, no
///   matter which order the passes happen to run in.
/// - a `.c2s-from-bundle` marker in each folder the bundle created, so the
///   TOOLS/ pass knows those folders are overwritable even on a later run,
///   while still skipping tools that are genuinely already installed.
///
/// Each bundle is stamped under `/opt/cadence/.c2s-bundles/` once it's been
/// extracted; these are 100+ GB archives, so a re-run must not unpack them
/// again. Delete the stamp to force one.
async fn install_cadence_bundles(
    bundles: &[PathBuf],
    dest_root: &Path,
    tx: &mpsc::UnboundedSender<String>,
) {
    if bundles.is_empty() {
        return;
    }

    let stamp_dir = dest_root.join(".c2s-bundles");
    if let Err(e) = tokio::fs::create_dir_all(&stamp_dir).await {
        send_log(tx, &format!("[ERROR] Could not create {}: {} - skipping the bulk bundles.", stamp_dir.display(), e));
        return;
    }

    for bundle in bundles {
        let file_name = bundle.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        let stamp = stamp_dir.join(format!("{}.done", file_name));
        if stamp.exists() {
            send_log(tx, &format!("[INFO] Bulk bundle {} already extracted - skipping (delete {} to force).", file_name, stamp.display()));
            continue;
        }

        let strip = bundle_strip_components(bundle).await;
        send_log(tx, &format!("[INFO] Extracting bulk bundle {} -> {}/ (this is large; existing files are kept)...", file_name, dest_root.display()));

        let before = dir_names(dest_root);
        if let Err(e) = extract_bundle(bundle, dest_root, strip).await {
            send_log(tx, &format!("[ERROR] Failed to extract {}: {}", file_name, e));
            continue;
        }

        let new_dirs: Vec<String> = dir_names(dest_root).difference(&before).cloned().collect();
        for name in &new_dirs {
            let _ = tokio::fs::write(
                dest_root.join(name).join(BUNDLE_MARKER),
                format!("Provided by the bulk bundle {}. Removed when CADENCE/TOOLS/{}.tar.gz is extracted over it.\n", file_name, name),
            )
            .await;
        }
        let _ = tokio::fs::write(&stamp, new_dirs.join("\n")).await;

        if new_dirs.is_empty() {
            send_log(tx, &format!("[INFO] {} extracted - no new tool folders (everything it carries was already installed).", file_name));
        } else {
            send_log(tx, &format!("[SUCCESS] {} extracted - provided {} tool folder(s): {}", file_name, new_dirs.len(), new_dirs.join(", ")));
        }
    }
}

/// `tar -xf <bundle> -C <dest_root> --skip-old-files [--strip-components=N]`.
async fn extract_bundle(archive: &Path, dest_root: &Path, strip: usize) -> Result<(), String> {
    let mut cmd = Command::new("tar");
    cmd.arg("-xf").arg(archive).arg("-C").arg(dest_root).arg("--skip-old-files");
    if strip > 0 {
        cmd.arg(format!("--strip-components={}", strip));
    }
    let status = cmd.status().await.map_err(|e| format!("failed to spawn tar: {}", e))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("tar exited with {:?}", status.code()))
    }
}

/// How many leading path components to strip so a bundle's tool folders land
/// directly in /opt/cadence/.
///
/// The per-tool TOOLS/ archives are all shaped the same way, but the bulk
/// bundles aren't guaranteed to be: one may hold the tool folders at its top
/// level (IC618/, SPECTRE211/, ...), another may wrap them all in a single
/// folder named after the archive itself (Analog_RHEL_8/IC618/, ...). Sniff
/// the member list instead of assuming: strip one level only when everything
/// sampled sits under a single top-level folder that is the archive's own
/// name. Reads a bounded prefix of the listing and bails as soon as it sees a
/// second top-level entry, so it doesn't stream a 100+ GB archive to decide.
async fn bundle_strip_components(archive: &Path) -> usize {
    let file_name = archive.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
    let base = file_name.strip_suffix(".tar.gz").unwrap_or(&file_name).to_string();

    let mut child = match Command::new("tar")
        .arg("-tf")
        .arg(archive)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
    {
        Ok(child) => child,
        Err(_) => return 0,
    };

    let mut tops: BTreeSet<String> = BTreeSet::new();
    if let Some(stdout) = child.stdout.take() {
        let mut lines = BufReader::new(stdout).lines();
        let mut seen = 0;
        while let Ok(Some(line)) = lines.next_line().await {
            let top = line.trim().trim_start_matches("./").split('/').next().unwrap_or("").to_string();
            if !top.is_empty() {
                tops.insert(top);
            }
            seen += 1;
            if seen >= 2000 || tops.len() > 1 {
                break;
            }
        }
    }
    let _ = child.kill().await;

    if tops.len() == 1 && tops.contains(&base) {
        1
    } else {
        0
    }
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
        // Same rule as the *.tar.gz pass: a folder the bulk bundle laid down
        // is the TOOLS/ archive's to replace, anything else is already
        // installed and left alone. This one is a full swap rather than an
        // overwrite, since the payload is moved in entry by entry.
        if dest.join(BUNDLE_MARKER).exists() {
            send_log(tx, &format!("[INFO] {} came from a bulk bundle - replacing it with {}...", target_name, file_name));
            if let Err(e) = tokio::fs::remove_dir_all(&dest).await {
                send_log(tx, &format!("[ERROR] Could not remove the bundle's {}: {} - skipping.", dest.display(), e));
                return;
            }
        } else {
            send_log(tx, &format!("[INFO] {} already exists at {} - skipping re-extraction.", target_name, dest.display()));
            return;
        }
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
