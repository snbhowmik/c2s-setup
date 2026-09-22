use std::sync::{Arc, Mutex};
use std::path::Path;
use std::process::Command;

#[derive(Debug, Clone)]
pub struct ValidationState {
    pub is_checking: bool,
    pub prereqs_met: bool,
    pub essential_apps_met: bool,
    pub prereqs_details: Vec<(String, bool)>,
    pub apps_details: Vec<(String, bool)>,
    pub tool_binary_status: Vec<(String, bool)>,
    pub tool_env_status: Vec<(String, bool)>,
    pub configured_users: Vec<(String, bool)>,
}

impl ValidationState {
    pub fn new() -> Self {
        Self {
            is_checking: true,
            prereqs_met: false,
            essential_apps_met: false,
            prereqs_details: vec![],
            apps_details: vec![],
            tool_binary_status: vec![
                ("Xilinx".to_string(), false),
                ("Cadence".to_string(), false),
                ("Silvaco".to_string(), false),
                ("CADRE".to_string(), false),
            ],
            tool_env_status: vec![
                ("Xilinx".to_string(), false),
                ("Cadence".to_string(), false),
                ("Silvaco".to_string(), false),
                ("CADRE".to_string(), false),
            ],
            configured_users: vec![],
        }
    }
}

pub fn spawn_system_validation(state: Arc<Mutex<ValidationState>>) {
    tokio::spawn(async move {
        // 1. Check Pre-requisites
        let prereqs = vec!["epel-release", "gcc", "libXp", "libnsl"];
        let mut prereqs_details = Vec::new();
        let mut all_prereqs = true;
        for pkg in prereqs {
            let output = Command::new("rpm").args(&["-q", pkg]).output();
            let is_installed = if let Ok(out) = output { out.status.success() } else { false };
            prereqs_details.push((pkg.to_string(), is_installed));
            if !is_installed {
                all_prereqs = false;
            }
        }

        // 2. Check Essential Apps
        let apps = vec![("Google Chrome", "google-chrome"), ("VS Code", "code"), ("AnyDesk", "anydesk")];
        let mut apps_details = Vec::new();
        let mut all_apps = true;
        for (name, bin) in apps {
            let output = Command::new("which").arg(bin).output();
            let is_installed = if let Ok(out) = output { out.status.success() } else { false };
            apps_details.push((name.to_string(), is_installed));
            if !is_installed {
                all_apps = false;
            }
        }

        // 3. Check Tools
        let tools = vec![
            ("Xilinx", "/opt/Xilinx"),
            ("Cadence", "/opt/cadence"),
            ("Silvaco", "/opt/sedatools"),
            ("CADRE", "/opt/CADRE")
        ];
        let mut bin_status = Vec::new();
        let mut env_status = Vec::new();
        for (name, path) in tools {
            let bin_exists = Path::new(path).exists();
            bin_status.push((name.to_string(), bin_exists));

            // Cadence keeps its own generated env file; Xilinx/Silvaco/CADRE are
            // defined entirely inside the shared eda-launcher.sh.
            let env_exists = if name.eq_ignore_ascii_case("Cadence") {
                Path::new(crate::installer::config::CADENCE_ENV_FILE).exists()
            } else {
                Path::new(crate::installer::config::EDA_LAUNCHER).exists()
            };
            env_status.push((name.to_string(), env_exists));
        }

        // 4. Check Configured Users - shared scan with the User Management
        // screen (user_mgr::list_lab_users) so the two never disagree.
        let users_list: Vec<(String, bool)> = crate::user_mgr::list_lab_users()
            .into_iter()
            .map(|u| (u.username, u.bashrc_configured))
            .collect();

        {
            let mut s = state.lock().unwrap();
            s.prereqs_details = prereqs_details;
            s.prereqs_met = all_prereqs;
            s.apps_details = apps_details;
            s.essential_apps_met = all_apps;
            s.tool_binary_status = bin_status;
            s.tool_env_status = env_status;
            s.configured_users = users_list;
            s.is_checking = false;
        }
    });
}
