#!/usr/bin/env bash
# =====================================================================
#  vnc-setup.sh - TigerVNC (systemd / vncsession) multi-user setup
#  Target: RHEL / Rocky / Alma 8+
#
#  - creates users + sets login password (idempotent, safe to re-run)
#  - copies .bashrc from srmist309x (or srmist309xx)
#  - sets the VNC password non-interactively (vncpasswd -f)
#  - assigns one FREE display per user (skips displays held by GDM/Xorg
#    or other users) and keeps other users' lines in vncserver.users
#  - enables the services so they survive a reboot, then validates them
#
#  Usage:  sudo bash vnc-setup.sh
# =====================================================================

set -uo pipefail
shopt -s nullglob

# ----------------------------- CONFIG --------------------------------
VNC_USERS=("yuvatsrm1" "yuvatsrm2")       # add more users here
USER_PASSWORD="srmist"                    # Linux login password
VNC_PASSWORD="srmist"                     # VNC password (only 8 chars used)
BASHRC_REFS=("srmist309x" "srmist309xx")  # first one that exists wins
ADD_TO_WHEEL="yes"                        # yes = give these users sudo
START_DISPLAY=1                           # first display to try (:1 = 5901)
GEOMETRY="1920x1080"
LOCALHOST_ONLY="no"                       # yes = reachable only via SSH tunnel
# ---------------------------------------------------------------------

USERS_FILE="/etc/tigervnc/vncserver.users"
DEFAULTS_FILE="/etc/tigervnc/vncserver-config-defaults"
STAMP="$(date +%Y%m%d-%H%M%S)"

log()  { echo -e "\e[32m[+]\e[0m $*"; }
warn() { echo -e "\e[33m[!]\e[0m $*"; }
err()  { echo -e "\e[31m[x]\e[0m $*"; }
die()  { err "$*"; exit 1; }

[[ $EUID -eq 0 ]] || die "Run as root:  sudo bash $0"

home_of()  { getent passwd "$1" | cut -d: -f6; }
group_of() { id -gn "$1"; }

# ---------------------------------------------------------------------
# 1. TigerVNC + GNOME
# ---------------------------------------------------------------------
if rpm -q tigervnc-server &>/dev/null; then
    log "TigerVNC already installed: $(rpm -q tigervnc-server)"
else
    log "Installing tigervnc-server..."
    dnf install -y tigervnc-server || die "Failed to install tigervnc-server"
fi

if [[ ! -f /usr/share/xsessions/gnome.desktop ]]; then
    log "GNOME Xorg session not found, installing..."
    dnf install -y gnome-session-xsession gnome-terminal nautilus dbus-x11 \
        || die "Failed to install GNOME session packages"
fi
[[ -f /usr/share/xsessions/gnome.desktop ]] \
    || die "/usr/share/xsessions/gnome.desktop missing - install the 'Server with GUI' group"

# TigerVNC >= 1.13 moved per-user files from ~/.vnc to XDG dirs
if grep -q 'is deprecated' /usr/libexec/vncserver 2>/dev/null; then
    XDG_MODE=1; log "TigerVNC uses ~/.config/tigervnc (new layout)"
else
    XDG_MODE=0; log "TigerVNC uses ~/.vnc (legacy layout)"
fi

# ---------------------------------------------------------------------
# 2. Global defaults file
# ---------------------------------------------------------------------
mkdir -p /etc/tigervnc
[[ -f $DEFAULTS_FILE ]] && cp -a "$DEFAULTS_FILE" "$DEFAULTS_FILE.bak.$STAMP"
{
    echo "## Managed by vnc-setup.sh ($STAMP)"
    echo "session=gnome"
    echo "geometry=$GEOMETRY"
    echo "alwaysshared"
    echo "securitytypes=vncauth,tlsvnc"
    [[ $LOCALHOST_ONLY == "yes" ]] && echo "localhost"
} > "$DEFAULTS_FILE"
log "Wrote $DEFAULTS_FILE"

# ---------------------------------------------------------------------
# 3. Stop previous sessions of OUR users only (never touch other users)
# ---------------------------------------------------------------------
touch "$USERS_FILE"
log "Stopping old VNC sessions of: ${VNC_USERS[*]}"
for user in "${VNC_USERS[@]}"; do
    for d in $(sed -nE "s/^[[:space:]]*:([0-9]+)=${user}[[:space:]]*$/\1/p" "$USERS_FILE"); do
        systemctl stop "vncserver@:${d}.service" 2>/dev/null
        systemctl disable --quiet "vncserver@:${d}.service" 2>/dev/null
    done
    id "$user" &>/dev/null && pkill -u "$user" 2>/dev/null
done
sleep 1
for user in "${VNC_USERS[@]}"; do
    id "$user" &>/dev/null && pkill -KILL -u "$user" 2>/dev/null
done
systemctl reset-failed 'vncserver@*' &>/dev/null
sleep 1

# ---------------------------------------------------------------------
# 4. Users, passwords, .bashrc, VNC password
# ---------------------------------------------------------------------
BASHRC_SRC=""
for ref in "${BASHRC_REFS[@]}"; do
    if id "$ref" &>/dev/null && [[ -f "$(home_of "$ref")/.bashrc" ]]; then
        BASHRC_SRC="$(home_of "$ref")/.bashrc"
        break
    fi
done
if [[ -n $BASHRC_SRC ]]; then
    log "Reference .bashrc: $BASHRC_SRC"
else
    warn "No .bashrc found for ${BASHRC_REFS[*]} - keeping default .bashrc"
fi

VNC_HASH="$(mktemp)"
trap 'rm -f "$VNC_HASH"' EXIT
# -f = filter mode: plain password on stdin, hash on stdout, no prompts
printf '%s\n' "$VNC_PASSWORD" | vncpasswd -f > "$VNC_HASH" || die "vncpasswd -f failed"
[[ -s $VNC_HASH ]] || die "Generated VNC password file is empty"

for user in "${VNC_USERS[@]}"; do
    if id "$user" &>/dev/null; then
        log "User $user already exists"
    else
        useradd -m "$user" || die "useradd $user failed"
        log "Created user $user"
    fi
    echo "$user:$USER_PASSWORD" | chpasswd || die "chpasswd failed for $user"
    [[ $ADD_TO_WHEEL == "yes" ]] && usermod -aG wheel "$user"

    uhome="$(home_of "$user")"
    ugrp="$(group_of "$user")"

    [[ -n $BASHRC_SRC ]] && install -m 644 -o "$user" -g "$ugrp" "$BASHRC_SRC" "$uhome/.bashrc"

    if (( XDG_MODE )); then
        # retire the deprecated dir so the new one is actually used
        [[ -d "$uhome/.vnc" ]] && mv "$uhome/.vnc" "$uhome/.vnc.bak.$STAMP"
        mkdir -p "$uhome/.config/tigervnc"
        chown "$user:$ugrp" "$uhome/.config" "$uhome/.config/tigervnc"
        passdir="$uhome/.config/tigervnc"
    else
        mkdir -p "$uhome/.vnc"
        chown "$user:$ugrp" "$uhome/.vnc"
        passdir="$uhome/.vnc"
    fi
    chmod 700 "$passdir"
    install -m 600 -o "$user" -g "$ugrp" "$VNC_HASH" "$passdir/passwd"
    rm -f "$uhome"/.vnc/*.pid "$uhome"/.local/state/tigervnc/*.pid

    install -m 600 -o "$user" -g "$ugrp" /dev/null "$uhome/.Xauthority"

    # fix SELinux labels (mislabeled homes make vncsession die silently)
    command -v restorecon &>/dev/null && restorecon -RF "$uhome"

    log "Configured $user (login pw, VNC pw -> $passdir/passwd, .bashrc)"
done

# ---------------------------------------------------------------------
# 5. Pick one FREE display per user
# ---------------------------------------------------------------------
# Prints who holds display $1 and returns 0; returns 1 if it is free.
display_holder() {
    local n=$1 line="" pid="" lock="/tmp/.X${1}-lock"
    if [[ -f $lock ]]; then
        pid=$(tr -dc '0-9' < "$lock")
        [[ -n $pid ]] && ! kill -0 "$pid" 2>/dev/null && pid=""
    fi
    if [[ -z $pid ]]; then
        # catches filesystem AND abstract (@/tmp/.X11-unix/Xn) sockets
        line=$(ss -xlpH 2>/dev/null | grep -E "/tmp/\.X11-unix/X${n}([[:space:]]|$)" | head -n1)
        [[ -z $line ]] && line=$(ss -tlpnH "( sport = :$((5900+n)) or sport = :$((6000+n)) )" 2>/dev/null | head -n1)
        [[ -z $line ]] && return 1
        pid=$(grep -oE 'pid=[0-9]+' <<<"$line" | head -n1 | cut -d= -f2)
    fi
    if [[ -n $pid ]]; then
        echo "$(ps -o comm= -p "$pid" 2>/dev/null) (pid $pid, user $(ps -o user= -p "$pid" 2>/dev/null))"
    else
        echo "an unknown process"
    fi
    return 0
}

# Remove lock/socket files only when the owning process is dead.
clean_stale() {
    local n=$1 lock="/tmp/.X${1}-lock" pid
    [[ -f $lock ]] || return 0
    pid=$(tr -dc '0-9' < "$lock")
    if [[ -z $pid ]] || ! kill -0 "$pid" 2>/dev/null; then
        rm -f "$lock" "/tmp/.X11-unix/X${n}"
        warn "Removed stale lock for :$n"
    fi
}

claimed_by() { sed -nE "s/^[[:space:]]*:${1}=([^[:space:]#]+).*/\1/p" "$USERS_FILE" | head -n1; }

declare -A DISP
next=$START_DISPLAY
for user in "${VNC_USERS[@]}"; do
    while (( next < 100 )); do
        clean_stale "$next"
        owner=$(claimed_by "$next")
        if [[ -n $owner && ! " ${VNC_USERS[*]} " =~ " $owner " ]]; then
            warn ":$next is reserved for '$owner' in vncserver.users - skipping"
        elif holder=$(display_holder "$next"); then
            warn ":$next is already used by $holder - skipping"
        else
            break
        fi
        next=$((next + 1))
    done
    (( next < 100 )) || die "No free display found"
    DISP[$user]=$next
    log "Assigned :$next (port $((5900 + next))) -> $user"
    next=$((next + 1))
done

# ---------------------------------------------------------------------
# 6. Update vncserver.users (keep everyone else's entries)
# ---------------------------------------------------------------------
cp -a "$USERS_FILE" "$USERS_FILE.bak.$STAMP"
pattern="^[[:space:]]*:[0-9]+=($(IFS='|'; echo "${VNC_USERS[*]}"))[[:space:]]*$"
tmp=$(mktemp)
grep -vE "$pattern" "$USERS_FILE" > "$tmp"
for user in "${VNC_USERS[@]}"; do
    echo ":${DISP[$user]}=$user" >> "$tmp"
done
install -m 644 -o root -g root "$tmp" "$USERS_FILE"
rm -f "$tmp"
command -v restorecon &>/dev/null && restorecon -RF /etc/tigervnc
log "Active mappings in $USERS_FILE:"
grep -vE '^[[:space:]]*(#|$)' "$USERS_FILE" | sed 's/^/      /'

# ---------------------------------------------------------------------
# 7. Firewall (only when listening on the network)
# ---------------------------------------------------------------------
if [[ $LOCALHOST_ONLY != "yes" ]] && systemctl is-active --quiet firewalld; then
    for user in "${VNC_USERS[@]}"; do
        firewall-cmd --quiet --permanent --add-port="$((5900 + DISP[$user]))/tcp"
    done
    firewall-cmd --quiet --reload
    log "firewalld: opened VNC ports"
fi

# ---------------------------------------------------------------------
# 8. Enable (survives reboot) + start
# ---------------------------------------------------------------------
systemctl daemon-reload
for user in "${VNC_USERS[@]}"; do
    d=${DISP[$user]}
    systemctl enable --quiet "vncserver@:${d}.service"
    systemctl restart "vncserver@:${d}.service"
done

# ---------------------------------------------------------------------
# 9. Validate (service active AND port listening AND still alive later)
# ---------------------------------------------------------------------
is_up() {
    local d=$1
    systemctl is-active --quiet "vncserver@:${d}.service" &&
        ss -tlnH "sport = :$((5900 + d))" | grep -q .
}

show_logs() {
    local user=$1 d=$2 uhome f
    uhome=$(home_of "$user")
    echo "----- journalctl vncserver@:$d -----"
    journalctl -u "vncserver@:${d}.service" -n 15 --no-pager
    for f in "$uhome"/.local/state/tigervnc/*:"$d".log "$uhome"/.vnc/*:"$d".log; do
        echo "----- $f -----"
        tail -n 25 "$f"
    done
}

log "Validating..."
FAIL=0
for user in "${VNC_USERS[@]}"; do
    d=${DISP[$user]}
    ok=0
    for _ in $(seq 1 25); do
        is_up "$d" && { ok=1; break; }
        sleep 1
    done
    # GNOME can die a few seconds after start - confirm it stays up
    if (( ok )); then sleep 8; is_up "$d" || ok=0; fi

    if (( ok )); then
        log "$user -> :$d (port $((5900 + d))) RUNNING, boot: $(systemctl is-enabled "vncserver@:${d}.service")"
    else
        err "$user -> :$d FAILED"
        show_logs "$user" "$d"
        FAIL=1
    fi
done

# ---------------------------------------------------------------------
# 10. Summary
# ---------------------------------------------------------------------
host=$(hostname -f 2>/dev/null || hostname)
echo
if (( FAIL == 0 )); then
    log "All sessions are up and enabled - they will start again after reboot."
    for user in "${VNC_USERS[@]}"; do
        d=${DISP[$user]}
        if [[ $LOCALHOST_ONLY == "yes" ]]; then
            echo "      $user: ssh -L $((5900 + d)):localhost:$((5900 + d)) $user@$host  then  vncviewer localhost:$d"
        else
            echo "      $user: vncviewer $host:$d"
        fi
    done
else
    err "Some sessions failed - logs are printed above."
    exit 1
fi
