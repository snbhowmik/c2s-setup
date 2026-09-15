use clap::Parser;
use tokio::sync::mpsc;

use crate::installer::config::LabConfig;
use crate::installer::{dependency, launcher, preinstall, tools};
use crate::user_mgr;

const KNOWN_TOOLS: &[&str] = &["xilinx", "cadence", "silvaco", "cadre"];

/// C2S EDA Lab Installer. Run with no arguments for the interactive TUI, or
/// pass any of these flags to perform actions headlessly - e.g. over SSH
/// scripted across the 20 lab workstations - without launching it.
#[derive(Parser, Debug)]
#[command(name = "c2s-setup", version, about)]
pub struct Cli {
    /// Set the machine number (1-20) before running any other action.
    #[arg(long, value_name = "N")]
    pub machine: Option<u8>,

    /// Print current install status (phase completion per tool) and exit.
    #[arg(long)]
    pub status: bool,

    /// Run Phase 0 pre-installation (system packages + student user + GDM/limits).
    #[arg(long)]
    pub preinstall: bool,

    /// Install one or more tools: xilinx, cadence, silvaco, cadre, or all.
    /// Repeatable (--install cadence --install xilinx) or comma-separated.
    #[arg(long, value_delimiter = ',', value_name = "TOOL")]
    pub install: Vec<String>,

    /// Regenerate the environment script(s) for one or more tools (same names as --install).
    #[arg(long = "recreate-env", value_delimiter = ',', value_name = "TOOL")]
    pub recreate_env: Vec<String>,

    /// Add/configure a lab user: USERNAME[,ROLE,REGISTER_NO]. Repeatable.
    #[arg(long = "add-user", value_name = "USERNAME[,ROLE,REG_NO]")]
    pub add_user: Vec<String>,

    /// Resolve and install a missing system dependency (package name or .so filename).
    #[arg(long, value_name = "PACKAGE")]
    pub dependency: Option<String>,
}

impl Cli {
    /// Any flag at all switches to headless CLI mode; a bare invocation (no
    /// flags) launches the interactive TUI, matching normal CLI conventions.
    pub fn wants_cli_mode(&self) -> bool {
        self.machine.is_some()
            || self.status
            || self.preinstall
            || !self.install.is_empty()
            || !self.recreate_env.is_empty()
            || !self.add_user.is_empty()
            || self.dependency.is_some()
    }
}

pub async fn run(cli: Cli, mut config: LabConfig) -> Result<(), String> {
    // Validate every tool name up front so a typo fails fast instead of
    // partway through a multi-tool run.
    let install_tools = resolve_tool_names(&cli.install)?;
    let recreate_env_tools = resolve_tool_names(&cli.recreate_env)?;

    let (tx, mut rx) = mpsc::unbounded_channel::<String>();
    let printer = tokio::spawn(async move {
        while let Some(msg) = rx.recv().await {
            println!("{}", msg);
        }
    });

    if let Some(n) = cli.machine {
        if !(1..=20).contains(&n) {
            return Err(format!("--machine must be between 1 and 20, got {}", n));
        }
        let sudo_user = std::env::var("SUDO_USER").unwrap_or_else(|_| "sysadmin".to_string());
        config = LabConfig::new(n, &sudo_user);
        config.save_state_key("MACHINE_NUMBER", &n.to_string()).map_err(|e| e.to_string())?;
        println!("[CONFIG] Machine set to #{} ({})", n, config.hostname_fqdn);
    }

    if cli.status {
        print_status(&config);
    }

    if cli.preinstall {
        preinstall::run_preinstall(&mut config, tx.clone()).await?;
    }

    for tool in &install_tools {
        install_one(tool, &mut config, tx.clone()).await?;
    }

    for tool in &recreate_env_tools {
        launcher::recreate_env(tool, tx.clone()).await?;
    }

    for spec in &cli.add_user {
        add_user_from_spec(spec, tx.clone()).await?;
    }

    if let Some(pkg) = &cli.dependency {
        dependency::resolve_and_install_dependency(pkg, tx.clone()).await?;
    }

    drop(tx);
    let _ = printer.await;
    Ok(())
}

async fn install_one(tool: &str, config: &mut LabConfig, tx: mpsc::UnboundedSender<String>) -> Result<(), String> {
    match tool {
        "xilinx" => tools::install_xilinx(config, tx).await,
        "cadence" => tools::install_cadence(config, tx).await,
        "silvaco" => {
            tools::install_silvaco(config, 1, tx.clone()).await?;
            tools::install_silvaco(config, 2, tx.clone()).await?;
            tools::install_silvaco(config, 3, tx).await
        }
        "cadre" => tools::install_cadre(config, tx).await,
        other => Err(format!("Unknown tool '{}'", other)),
    }
}

/// Expands "all" and validates every entry up front against KNOWN_TOOLS,
/// case-insensitively - a scripted/unattended run should fail loudly on a
/// typo'd tool name rather than silently skip it partway through.
fn resolve_tool_names(raw: &[String]) -> Result<Vec<String>, String> {
    let mut resolved = Vec::new();
    for entry in raw {
        let lower = entry.trim().to_lowercase();
        if lower == "all" {
            for t in KNOWN_TOOLS {
                resolved.push(t.to_string());
            }
            continue;
        }
        if !KNOWN_TOOLS.contains(&lower.as_str()) {
            return Err(format!(
                "Unknown tool '{}' - expected one of: {}, or all",
                entry,
                KNOWN_TOOLS.join(", ")
            ));
        }
        resolved.push(lower);
    }
    Ok(resolved)
}

async fn add_user_from_spec(spec: &str, tx: mpsc::UnboundedSender<String>) -> Result<(), String> {
    let parts: Vec<&str> = spec.split(',').map(|s| s.trim()).collect();
    let (username, role, identifier) = match parts.as_slice() {
        [u, r, id] => (u.to_string(), r.to_string(), id.to_string()),
        [u] => (u.to_string(), "Student".to_string(), "REG-DEFAULT".to_string()),
        _ => return Err(format!("Invalid --add-user format '{}'. Use USERNAME[,ROLE,REGISTER_NO]", spec)),
    };
    user_mgr::create_or_configure_student_user(&username, &role, &identifier, tx).await
}

fn print_status(config: &LabConfig) {
    println!("Machine: #{} ({})", config.machine_number, config.hostname_fqdn);
    println!("Sysadmin user: {}", config.sysadmin_user);
    println!("Student user:  {}", config.student_user);
    println!();
    for phase in ["PRE_INSTALL", "XILINX", "CADENCE", "SILVACO_1", "SILVACO_2", "SILVACO_3", "CADRE"] {
        let done = config.is_phase_done(phase);
        let time = config.phase_time(phase).unwrap_or_else(|| "-".to_string());
        println!("  [{}] {:<12} {}", if done { "x" } else { " " }, phase, time);
    }
}
