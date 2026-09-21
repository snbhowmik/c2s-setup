use std::collections::HashMap;
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use chrono::Local;

pub const STATE_DIR: &str = "/var/log/vlsilab";
pub const STATE_FILE: &str = "/var/log/vlsilab/install.state";
pub const LOG_FILE: &str = "/var/log/vlsilab/install.log";
pub const EDA_LAUNCHER: &str = "/opt/vlsilab/eda-launcher.sh";
pub const CADENCE_ENV_FILE: &str = "/opt/cadence/cadence-env.sh";

#[derive(Debug, Clone)]
pub struct LabConfig {
    pub machine_number: u8,
    pub sysadmin_user: String,
    pub student_user: String,
    pub hostname_fqdn: String,
    pub state: HashMap<String, String>,
}

impl LabConfig {
    pub fn new(machine_num: u8, _current_user: &str) -> Self {
        let sysadmin_user = format!("sysadmin309{}", machine_num);
        let student_user = format!("srmist309{}", machine_num);
        let hostname_fqdn = format!("vlsilab{}.ist.srmtrichy.edu.in", machine_num);
        
        let mut config = Self {
            machine_number: machine_num,
            sysadmin_user,
            student_user,
            hostname_fqdn,
            state: HashMap::new(),
        };
        config.load_state();
        config
    }

    pub fn load_from_state(_current_user: &str) -> Option<Self> {
        if !Path::new(STATE_FILE).exists() {
            return None;
        }

        let content = fs::read_to_string(STATE_FILE).ok()?;
        let mut state = HashMap::new();

        for line in content.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            if let Some((k, v)) = line.split_once('=') {
                state.insert(k.trim().to_string(), v.trim().to_string());
            }
        }

        let machine_num: u8 = state.get("MACHINE_NUMBER")?.parse().ok()?;
        let sysadmin_user = state.get("SYSADMIN_USER").cloned().unwrap_or_else(|| format!("sysadmin309{}", machine_num));
        let student_user = state.get("STUDENT_USER").cloned().unwrap_or_else(|| format!("srmist309{}", machine_num));
        let hostname_fqdn = state.get("HOSTNAME").cloned().unwrap_or_else(|| format!("vlsilab{}.ist.srmtrichy.edu.in", machine_num));

        Some(Self {
            machine_number: machine_num,
            sysadmin_user,
            student_user,
            hostname_fqdn,
            state,
        })
    }

    /// Always the *current* process working directory - never persisted or
    /// cached. This used to be read from install.state once written by an
    /// earlier run, which meant it silently kept pointing at wherever the
    /// installer happened to be the very first time it ran (e.g. a USB drive
    /// mount path) forever after, even once the tool folders had moved
    /// somewhere else entirely. "Where am I running from right now" is not a
    /// durable setting like the machine number - it must be re-read every time.
    pub fn get_script_dir(&self) -> PathBuf {
        std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
    }

    /// Resolves the ROOT directory containing the tool folders (CADENCE, SILVACO,
    /// XILINX, CADRE, SYNOPSYS). setup.sh validates those as direct children of
    /// wherever it was invoked from and launches the TUI with that same cwd, so
    /// ROOT is SCRIPT_DIR itself — not its parent.
    pub fn get_root_dir(&self) -> PathBuf {
        self.get_script_dir()
    }

    pub fn get_tool_dir(&self, tool_name: &str) -> PathBuf {
        self.get_root_dir().join(tool_name)
    }

    pub fn load_state(&mut self) {
        if !Path::new(STATE_FILE).exists() {
            return;
        }

        if let Ok(content) = fs::read_to_string(STATE_FILE) {
            for line in content.lines() {
                let line = line.trim();
                if line.is_empty() || line.starts_with('#') {
                    continue;
                }
                if let Some((k, v)) = line.split_once('=') {
                    self.state.insert(k.trim().to_string(), v.trim().to_string());
                }
            }
        }
    }

    pub fn save_state_key(&mut self, key: &str, val: &str) -> io::Result<()> {
        self.state.insert(key.to_string(), val.to_string());
        
        let _ = fs::create_dir_all(STATE_DIR);
        let mut file = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(STATE_FILE)?;

        for (k, v) in &self.state {
            writeln!(file, "{}={}", k, v)?;
        }
        
        if let Ok(site_config_dir) = std::env::var("VLSI_SITE_CONFIG") {
            let sync_dir = Path::new(&site_config_dir).join("machine_states");
            let _ = fs::create_dir_all(&sync_dir);
            let sync_file = sync_dir.join(format!("machine_{}.state", self.machine_number));
            let _ = fs::copy(STATE_FILE, sync_file);
        }

        Ok(())
    }

    pub fn mark_phase_done(&mut self, phase: &str) -> io::Result<()> {
        self.save_state_key(phase, "DONE")?;
        let now = Local::now().format("%Y-%m-%d %H:%M:%S").to_string();
        self.save_state_key(&format!("{}_TIME", phase), &now)?;
        self.append_log(&format!("[INFO] Phase complete: {}", phase))?;
        Ok(())
    }

    pub fn is_phase_done(&self, phase: &str) -> bool {
        self.state.get(phase).map(|s| s == "DONE").unwrap_or(false)
    }

    pub fn phase_time(&self, phase: &str) -> Option<String> {
        self.state.get(&format!("{}_TIME", phase)).cloned()
    }

    pub fn append_log(&self, msg: &str) -> io::Result<()> {
        let _ = fs::create_dir_all(STATE_DIR);
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(LOG_FILE)?;
        let timestamp = Local::now().format("%Y-%m-%d %H:%M:%S").to_string();
        writeln!(file, "{} - {}", timestamp, msg)?;
        Ok(())
    }
}
