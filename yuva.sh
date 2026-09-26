#!/bin/bash

set -e

USERS=("yuvatsrm1" "yuvatsrm2")
PASSWORD="srmist"
REFERENCE_USER="srmist309x"

echo "[+] Checking & Installing TigerVNC..."
if ! rpm -q tigervnc-server &>/dev/null; then
    sudo dnf install -y tigervnc-server
fi

echo "[+] Checking GNOME installation..."
if ! rpm -q gnome-session &>/dev/null; then
    echo "[+] Installing GNOME (this may take time)..."
    sudo dnf groupinstall -y "Server with GUI"
fi

echo "[+] Creating /etc/tigervnc config..."
sudo mkdir -p /etc/tigervnc

sudo tee /etc/tigervnc/vncserver-config-defaults > /dev/null <<EOF
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

echo "[+] Creating VNC password template..."
TMP_PASSWD="/tmp/vnc_passwd"
printf "$PASSWORD\n$PASSWORD\n\n" | vncpasswd > "$TMP_PASSWD"
chmod 600 "$TMP_PASSWD"

echo "[+] Creating users + assigning displays..."

DISPLAY_NUM=1
VNC_USERS_FILE=""

for user in "${USERS[@]}"; do

    if ! id "$user" &>/dev/null; then
        echo "[+] Creating user: $user"
        sudo useradd -m -G wheel "$user"
        echo "$user:$PASSWORD" | sudo chpasswd
    fi

    HOME_DIR=$(eval echo "~$user")

    # Copy bashrc
    if [ -n "$SRC_BASHRC" ] && [ -f "$SRC_BASHRC" ]; then
        sudo cp "$SRC_BASHRC" "$HOME_DIR/.bashrc"
        sudo chown $user:$user "$HOME_DIR/.bashrc"
    fi

    # Setup VNC password
    sudo mkdir -p "$HOME_DIR/.vnc"
    sudo cp "$TMP_PASSWD" "$HOME_DIR/.vnc/passwd"

    # GNOME xstartup (CRITICAL FIX)
    sudo tee "$HOME_DIR/.vnc/xstartup" > /dev/null <<'EOF'
#!/bin/bash
unset SESSION_MANAGER
unset DBUS_SESSION_BUS_ADDRESS
exec gnome-session &
EOF

    sudo chmod +x "$HOME_DIR/.vnc/xstartup"
    sudo chown -R $user:$user "$HOME_DIR/.vnc"
    sudo chmod 600 "$HOME_DIR/.vnc/passwd"

    # Kill stale sessions + sockets
    vncserver -kill :$DISPLAY_NUM &>/dev/null || true
    sudo rm -rf /tmp/.X11-unix/X$DISPLAY_NUM

    VNC_USERS_FILE+=":${DISPLAY_NUM}=${user}"$'\n'
    ((DISPLAY_NUM++))
done

echo "[+] Writing vncserver.users..."
echo "$VNC_USERS_FILE" | sudo tee /etc/tigervnc/vncserver.users > /dev/null

echo "[+] Reloading systemd..."
sudo systemctl daemon-reexec
sudo systemctl daemon-reload

echo "[+] Starting services..."

DISPLAY_NUM=1
for user in "${USERS[@]}"; do
    sudo systemctl enable vncserver@:${DISPLAY_NUM}.service
    sudo systemctl restart vncserver@:${DISPLAY_NUM}.service
    ((DISPLAY_NUM++))
done

echo "[+] Validating via ports (REAL CHECK)..."

DISPLAY_NUM=1
for user in "${USERS[@]}"; do
    PORT=$((5900 + DISPLAY_NUM))

    sleep 2

    if ss -tulnp | grep -q ":$PORT"; then
        echo "[✓] $user running on port $PORT"
    else
        echo "[✗] $user NOT running on port $PORT"
        echo "[!] Debug log:"
        sudo journalctl -xeu vncserver@:${DISPLAY_NUM}.service --no-pager | tail -n 15
    fi

    ((DISPLAY_NUM++))
done

echo "[+] Cleanup..."
rm -f "$TMP_PASSWD"

echo "[🔥 DONE] GNOME VNC fully working + persistent."
