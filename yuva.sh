#!/bin/bash

set -e

USERS=("yuvatsrm1" "yuvatsrm2")
PASSWORD="srmist"
REFERENCE_USER="srmist309x"

MAX_RETRIES=3

log() { echo -e "\e[32m[+]\e[0m $1"; }
warn() { echo -e "\e[33m[!]\e[0m $1"; }
err() { echo -e "\e[31m[✗]\e[0m $1"; }

# --------------------------------------------------
# 1. Install dependencies
# --------------------------------------------------
log "Checking dependencies..."

sudo dnf install -y tigervnc-server dbus-x11 xorg-x11-xauth >/dev/null

if ! rpm -q gnome-session &>/dev/null; then
    log "Installing GNOME..."
    sudo dnf groupinstall -y "Server with GUI"
fi

# --------------------------------------------------
# 2. Config files
# --------------------------------------------------
log "Configuring TigerVNC..."

sudo mkdir -p /etc/tigervnc

sudo tee /etc/tigervnc/vncserver-config-defaults >/dev/null <<EOF
session=gnome
geometry=1920x1080
localhost
alwaysshared
EOF

# Detect bashrc source
if id "$REFERENCE_USER" &>/dev/null; then
    SRC_BASHRC="/home/$REFERENCE_USER/.bashrc"
elif id "srmist309xx" &>/dev/null; then
    SRC_BASHRC="/home/srmist309xx/.bashrc"
else
    SRC_BASHRC=""
fi

# --------------------------------------------------
# 3. Generate password file
# --------------------------------------------------
TMP_PASSWD="/tmp/vnc_passwd"
printf "$PASSWORD\n$PASSWORD\n\n" | vncpasswd > "$TMP_PASSWD"
chmod 600 "$TMP_PASSWD"

# --------------------------------------------------
# 4. Create users + setup
# --------------------------------------------------
DISPLAY_NUM=1
VNC_USERS_FILE=""

for user in "${USERS[@]}"; do

    if ! id "$user" &>/dev/null; then
        log "Creating user $user"
        sudo useradd -m -G wheel "$user"
        echo "$user:$PASSWORD" | sudo chpasswd
    fi

    HOME_DIR=$(eval echo "~$user")

    # bashrc copy
    if [ -n "$SRC_BASHRC" ] && [ -f "$SRC_BASHRC" ]; then
        sudo cp "$SRC_BASHRC" "$HOME_DIR/.bashrc"
        sudo chown $user:$user "$HOME_DIR/.bashrc"
    fi

    # VNC setup
    sudo mkdir -p "$HOME_DIR/.vnc"
    sudo cp "$TMP_PASSWD" "$HOME_DIR/.vnc/passwd"

    # GNOME FIXED xstartup
    sudo tee "$HOME_DIR/.vnc/xstartup" >/dev/null <<'EOF'
#!/bin/bash
unset SESSION_MANAGER
unset DBUS_SESSION_BUS_ADDRESS

export XDG_RUNTIME_DIR=/run/user/$(id -u)
export DBUS_SESSION_BUS_ADDRESS=unix:path=$XDG_RUNTIME_DIR/bus

exec dbus-launch --exit-with-session gnome-session
EOF

    sudo chmod +x "$HOME_DIR/.vnc/xstartup"
    sudo chown -R $user:$user "$HOME_DIR/.vnc"
    sudo chmod 600 "$HOME_DIR/.vnc/passwd"

    # Ensure Xauthority exists
    sudo -u $user touch "$HOME_DIR/.Xauthority"
    sudo chown $user:$user "$HOME_DIR/.Xauthority"

    VNC_USERS_FILE+=":${DISPLAY_NUM}=${user}"$'\n'
    ((DISPLAY_NUM++))
done

echo "$VNC_USERS_FILE" | sudo tee /etc/tigervnc/vncserver.users >/dev/null

# --------------------------------------------------
# 5. Self-healing start function
# --------------------------------------------------
fix_and_restart() {
    local display=$1
    local user=$2
    local home=$(eval echo "~$user")

    warn "Healing display :$display for $user"

    # Kill sessions
    vncserver -kill :$display &>/dev/null || true

    # Kill stray processes
    pkill -u $user Xvnc &>/dev/null || true

    # Clean locks
    sudo rm -rf /tmp/.X${display}-lock
    sudo rm -rf /tmp/.X11-unix/X${display}

    # Clean user VNC leftovers
    rm -rf "$home/.vnc/"*.pid "$home/.vnc/"*.log 2>/dev/null || true

    # Restart
    sudo systemctl restart vncserver@:${display}.service
}

# --------------------------------------------------
# 6. Start + self-heal loop
# --------------------------------------------------
log "Starting VNC services..."

sudo systemctl daemon-reexec
sudo systemctl daemon-reload

DISPLAY_NUM=1

for user in "${USERS[@]}"; do
    sudo systemctl enable vncserver@:${DISPLAY_NUM}.service
    sudo systemctl restart vncserver@:${DISPLAY_NUM}.service
    ((DISPLAY_NUM++))
done

# --------------------------------------------------
# 7. Validation + retry loop
# --------------------------------------------------
log "Validating services (self-healing mode)..."

DISPLAY_NUM=1

for user in "${USERS[@]}"; do
    PORT=$((5900 + DISPLAY_NUM))
    SUCCESS=0

    for ((i=1; i<=MAX_RETRIES; i++)); do
        sleep 2

        if ss -tulnp | grep -q ":$PORT"; then
            log "$user running on port $PORT"
            SUCCESS=1
            break
        else
            warn "$user failed (attempt $i), fixing..."
            fix_and_restart "$DISPLAY_NUM" "$user"
        fi
    done

    if [ $SUCCESS -eq 0 ]; then
        err "$user FAILED after retries"
        sudo journalctl -xeu vncserver@:${DISPLAY_NUM}.service --no-pager | tail -n 20
    fi

    ((DISPLAY_NUM++))
done

# --------------------------------------------------
# Cleanup
# --------------------------------------------------
rm -f "$TMP_PASSWD"

log "DONE — self-healing VNC setup complete."
