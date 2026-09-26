#!/bin/bash

set -e

USERS=("yuvatsrm1" "yuvatsrm2")
PASSWORD="srmist"
REFERENCE_USER="srmist309x"   # fallback handled below

echo "[+] Checking TigerVNC installation..."
if ! rpm -q tigervnc-server &>/dev/null; then
    echo "[+] Installing TigerVNC..."
    sudo dnf install -y tigervnc-server
else
    echo "[✓] TigerVNC already installed"
fi

echo "[+] Creating /etc/tigervnc config..."
sudo mkdir -p /etc/tigervnc

sudo tee /etc/tigervnc/vncserver-config-defaults > /dev/null <<EOF
session=gnome
geometry=1920x1080
localhost
alwaysshared
EOF

echo "[+] Detecting reference user for .bashrc..."
if id "$REFERENCE_USER" &>/dev/null; then
    SRC_BASHRC="/home/$REFERENCE_USER/.bashrc"
elif id "srmist309xx" &>/dev/null; then
    SRC_BASHRC="/home/srmist309xx/.bashrc"
else
    echo "[!] No reference user found, skipping bashrc copy"
    SRC_BASHRC=""
fi

echo "[+] Creating VNC password template..."
TMP_PASSWD="/tmp/vnc_passwd"
printf "$PASSWORD\n$PASSWORD\n\n" | vncpasswd > "$TMP_PASSWD"
chmod 600 "$TMP_PASSWD"

echo "[+] Creating users and assigning displays..."

DISPLAY_NUM=1
VNC_USERS_FILE=""

for user in "${USERS[@]}"; do

    if ! id "$user" &>/dev/null; then
        echo "[+] Creating user: $user"
        sudo useradd -m -G wheel "$user"
        echo "$user:$PASSWORD" | sudo chpasswd
    else
        echo "[✓] User $user already exists"
    fi

    HOME_DIR=$(eval echo "~$user")

    # Copy bashrc if available
    if [ -n "$SRC_BASHRC" ] && [ -f "$SRC_BASHRC" ]; then
        sudo cp "$SRC_BASHRC" "$HOME_DIR/.bashrc"
        sudo chown $user:$user "$HOME_DIR/.bashrc"
    fi

    # Setup VNC password
    sudo mkdir -p "$HOME_DIR/.vnc"
    sudo cp "$TMP_PASSWD" "$HOME_DIR/.vnc/passwd"
    sudo chown -R $user:$user "$HOME_DIR/.vnc"
    sudo chmod 600 "$HOME_DIR/.vnc/passwd"

    # Assign display
    VNC_USERS_FILE+=":${DISPLAY_NUM}=${user}"$'\n'

    ((DISPLAY_NUM++))
done

echo "[+] Writing /etc/tigervnc/vncserver.users..."
echo "$VNC_USERS_FILE" | sudo tee /etc/tigervnc/vncserver.users > /dev/null

echo "[+] Reloading systemd..."
sudo systemctl daemon-reexec
sudo systemctl daemon-reload

echo "[+] Enabling and starting VNC services..."

DISPLAY_NUM=1
for user in "${USERS[@]}"; do
    sudo systemctl enable vncserver@:${DISPLAY_NUM}.service
    sudo systemctl restart vncserver@:${DISPLAY_NUM}.service
    ((DISPLAY_NUM++))
done

echo "[+] Checking service status..."

DISPLAY_NUM=1
for user in "${USERS[@]}"; do
    if systemctl is-active --quiet vncserver@:${DISPLAY_NUM}.service; then
        echo "[✓] vncserver@:${DISPLAY_NUM} for $user is RUNNING"
    else
        echo "[✗] vncserver@:${DISPLAY_NUM} for $user FAILED"
        sudo journalctl -xeu vncserver@:${DISPLAY_NUM}.service --no-pager | tail -n 20
    fi
    ((DISPLAY_NUM++))
done

echo "[+] Cleanup..."
rm -f "$TMP_PASSWD"

echo "[🔥 DONE] VNC fully configured with users, passwords, and auto-start."
