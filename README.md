# C2S Setup Tool
**Author:** snbhowmik

---

## Overview

A unified interactive setup tool for configuring RHEL 8 workstations in C2S environments with EDA tools. The project consists of a lightweight `setup.sh` bootstrapper that automatically fetches a standalone Rust-based Terminal User Interface (TUI) to orchestrate and track installation progress.

### Supported Tools
| Tool | Status | Expected Source Directory |
|---|---|---|
| Xilinx Vivado/Vitis 2025.2 | ✅ Supported | `XILINX/` |
| Cadence (Analog + Digital) | ✅ Supported | `CADENCE/` |
| Silvaco TCAD Suite | ✅ Supported | `SILVACO/` |
| CADRE VisualTCAD | ✅ Supported | `CADRE/` |
| Synopsys | ⏳ Coming Soon | `SYNOPSYS/` |

---

## Folder Layout

Place your installer files alongside `setup.sh` when you have them. None of these folders are required just to run the installer — a missing one only means *that tool's* install action isn't available yet; everything else (pre-install, dependency resolution, user management, network diagnostics, env regeneration, and installing whichever tools you *do* have) still works normally. `setup.sh` prints a warning listing anything missing, not an error.

```
├── setup.sh
├── CADENCE/
│   └── TOOLS/
│       ├── ASSURA41.tar.gz
│       ├── GENUS211.tar.gz
│       ├── IC618.tar.gz
│       └── ... one .tar.gz per Cadence sub-tool
├── SILVACO/
│   ├── 243423-tcadlegacyandinterco-2024-00-rh64.bin
│   ├── 255020-victorytcad-2025-01-rh64.bin
│   └── 255017-victory_str-2025-01.bin
├── XILINX/
│   └── FPGAs_AdaptiveSoCs_...Lin64.bin (or extracted xsetup folder)
├── CADRE/
│   └── Cadre-VisualTCAD-Linux-2025.04.r3-284.bin
├── SYNOPSYS/
```

---

## Quick Start

The bootstrapper can be run directly via `curl` to ensure you always have the latest version. It will automatically download the compiled Rust TUI release, verify its SHA256 checksum, and launch the dashboard.

```bash
curl -fsSL https://raw.githubusercontent.com/snbhowmik/c2s-setup/main/setup.sh | sudo bash
```

The bootstrapper will:
1. Validate your directory structure.
2. Prompt for your Lab/Institution Name and Hostname format.
3. Fetch the latest `c2s-setup-linux-amd64` release binary from GitHub.
4. Launch the TUI Dashboard.

---

## The Rust TUI Dashboard

The core of the installation logic has been rewritten in Rust for speed and reliability. The TUI provides:
- Live log streaming
- Phase completion tracking
- Interactive dependency resolution for missing Linux libraries (`libpng12.so.0`, `libQt5Svg`, etc.)
- User and machine number management

### Quick Actions Panel:
- **`[0]`** System Pre-Install (Dependencies + Student User)
- **`[x]`**, **`[c]`**, **`[s]`**, **`[v]`** Install specific EDA tools
- **`[p]`** Solve Missing Dependencies automatically
- **`[m]`** Change Machine Number Config
- **`[u]`** Open User Management

---

## CLI / Headless Mode

The same binary also runs non-interactively - no TUI, no terminal UI at all - for scripting an install across the lab's workstations over SSH. Passing **any** flag switches it to this mode; a bare invocation still opens the TUI.

```bash
c2s-setup-linux-amd64 --status                                    # print phase-completion status and exit
c2s-setup-linux-amd64 --machine 5 --preinstall --install cadence  # set machine #, run pre-install, install Cadence
c2s-setup-linux-amd64 --install xilinx,silvaco,cadre              # install multiple tools (or --install all)
c2s-setup-linux-amd64 --recreate-env cadence                      # regenerate a tool's environment scripts
c2s-setup-linux-amd64 --add-user srmist30920,Student,RA2111003010001
c2s-setup-linux-amd64 --dependency libpng12.so.0
c2s-setup-linux-amd64 --help                                      # full flag reference
```

`setup.sh` forwards any arguments straight through, so the same flags work through the bootstrapper too - including piped through `curl`:

```bash
curl -fsSL https://raw.githubusercontent.com/snbhowmik/c2s-setup/main/setup.sh | sudo bash -s -- --preinstall --install cadence
```

---

## License Servers

| Tool | Port | Server |
|---|---|---|
| Xilinx | 2100 | 14.139.1.126 / c2s.cdacb.in |
| Cadence | 5280 | 14.139.1.126 / c2s.cdacb.in |
| Silvaco | 27000 | c2s.cdacb.in |
| CADRE | 20720, 20721 | c2s.cdacb.in |

---

*Author: snbhowmik | C2S Setup Tool*
