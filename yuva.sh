#!/bin/bash

set -e

USERS=("yuvatsrm1" "yuvatsrm2")
PASSWORD="srmist"
REFERENCE_USER="srmist309x"

log(){ echo -e "\e[32m[+]\e[0m $1"; }
warn(){ echo -e "\e[33m[!]\e[0m $1"; }
err(){ echo -e "\e[31m[✗]\e[0m $1"; }

# --------------------------------------------------
# 1. INSTALL EVERYTHING REQUIRED
# --------------------------------------------------
log "Installing dependencies..."

sudo dnf install -y tigervnc-server \
gnome-session gnome-session-xsession \
xorg-x11-server-Xorg xorg-x11-xauth \
dbus-x11 gnome-terminal nautilus >/dev/null

# --------------------------------------------------
# 2. FORCE XORG (disable Wayland)
# --------------------------------------------------
log "Disabling Wayland..."

sudo sed -i 's/#WaylandEnable=false/WaylandEnable=false/' /etc/gdm/custom.conf || true

# --------------------------------------------------
# 3. TIGERVNC CONFIG (vncsession mode)
# --------------------------------------------------
log "Configuring TigerVNC..."

sudo mkdir -p /etc/tigervnc

sudo tee /etc/tigervnc/vncserver-config-defaults >/dev/null <<EOF
session=gnome
geometry=1920x1080
localhost
alwaysshared
securitytypes=vncauth,tlsvnc
EOF

# --------------------------------------------------
# 4. CREATE USERS + PASSWORD
# --------------------------------------------------
log "Creating users..."

TMP_PASSWD="/tmp/vnc_passwd"
printf "$PASSWORD\n$PASSWORD\n\n" | vncpasswd > "$TMP_PASSWD"
chmod 600 "$TMP_PASSWD"

DISPLAY=1
VNC_USERS=""

for user in "${USERS[@]}"; do

    if ! id "$user" &>/dev/null; then
        sudo useradd -m -G wheel "$user"
        echo "$user:$PASSWORD" | sudo chpasswd
    fi

    HOME=$(eval echo "~$user")

    # Copy bashrc if exists
    if id "$REFERENCE_USER" &>/dev/null; then
        sudo cp /home/$REFERENCE_USER/.bashrc $HOME/.bashrc
    elif id "srmist309xx" &>/dev/null; then
        sudo cp /home/srmist309xx/.bashrc $HOME/.bashrc
    fi

    # Setup VNC passwd
    sudo mkdir -p $HOME/.vnc
    sudo cp $TMP_PASSWD $HOME/.vnc/passwd

    sudo chown -R $user:$user $HOME/.vnc $HOME/.bashrc 2>/dev/null || true
    sudo chmod 600 $HOME/.vnc/passwd

    # Ensure Xauthority exists
    sudo -u $user touch $HOME/.Xauthority

    VNC_USERS+=":${DISPLAY}=${user}"$'\n'
    ((DISPLAY++))
done

echo "$VNC_USERS" | sudo tee /etc/tigervnc/vncserver.users >/dev/null

# --------------------------------------------------
# 5. HARD CLEAN (CRITICAL)
# --------------------------------------------------
log "Cleaning old sessions..."

sudo systemctl stop vncserver@:1.service vncserver@:2.service 2>/dev/null || true

sudo pkill -9 Xvnc 2>/dev/null || true

sudo rm -rf /tmp/.X*
sudo rm -rf /tmp/.X11-unix/*

for user in "${USERS[@]}"; do
    rm -rf /home/$user/.vnc/*.log /home/$user/.vnc/*.pid 2>/dev/null || true
done

# --------------------------------------------------
# 6. START SERVICES
# --------------------------------------------------
log "Starting VNC services..."

sudo systemctl daemon-reexec
sudo systemctl daemon-reload

DISPLAY=1
for user in "${USERS[@]}"; do
    sudo systemctl enable vncserver@:${DISPLAY}.service
    sudo systemctl restart vncserver@:${DISPLAY}.service
    ((DISPLAY++))
done

# --------------------------------------------------
# 7. VALIDATION (REAL CHECK)
# --------------------------------------------------
log "Validating ports..."

sleep 5

DISPLAY=1
FAIL=0

for user in "${USERS[@]}"; do
    PORT=$((5900 + DISPLAY))

    if ss -tulnp | grep -q ":$PORT"; then
        log "$user running on port $PORT"
    else
        err "$user FAILED on port $PORT"
        FAIL=1
    fi

    ((DISPLAY++))
done

# --------------------------------------------------
# 8. FINAL STATUS
# --------------------------------------------------
if [ $FAIL -eq 0 ]; then
    log "🔥 SUCCESS — GNOME VNC WORKING"
else
    err "❌ STILL FAILING — showing logs"

    sudo journalctl -xeu vncserver@:1.service --no-pager | tail -n 20
    sudo journalctl -xeu vncserver@:2.service --no-pager | tail -n 20
fi

rm -f $TMP_PASSWD
