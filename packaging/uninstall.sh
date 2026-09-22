#!/usr/bin/env bash
# alienfan uninstaller (SPEC 11.5): undoes install.sh in reverse order.
#
# Scope: what install.sh installs today. It grows with the installer.
set -euo pipefail

GROUP=alienfan
ETC=/etc/alienfan

say() { printf '\n==> %s\n' "$*"; }
run_sudo() { printf '+ sudo %s\n' "$*"; sudo "$@"; }
ask() { local reply; read -r -p "$1 [s/N] " reply; [[ $reply == [sS]* ]]; }

[[ $EUID -ne 0 ]] || { echo "rode como usuário, sem sudo" >&2; exit 1; }

say "Daemon da sessão"
if [[ -e /usr/lib/systemd/user/alienfand.service ]]; then
  systemctl --user disable --now alienfand.service || true
  run_sudo rm -f /usr/lib/systemd/user/alienfand.service \
    /usr/share/dbus-1/services/io.github.lorenzopasquali.AlienFan.service
  systemctl --user daemon-reload
fi

say "Serviço de boot"
if [[ -e /etc/systemd/system/alienfan-apply.service ]]; then
  run_sudo systemctl disable --now alienfan-apply.service
  run_sudo rm -f /etc/systemd/system/alienfan-apply.service
  run_sudo systemctl daemon-reload
fi

say "Regra udev e permissões dos nós"
run_sudo rm -f /etc/udev/rules.d/71-alienfan.rules
run_sudo udevadm control --reload
# The rule changed the nodes in place; a trigger alone does not undo it.
P=$(grep -lx alienware-wmi /sys/class/platform-profile/*/name 2>/dev/null | xargs -r dirname)
H=$(grep -lx alienware_wmi /sys/class/hwmon/*/name 2>/dev/null | xargs -r dirname)
for node in ${P:+$P/profile} ${H:+$H/fan1_boost $H/fan2_boost}; do
  run_sudo chgrp root "$node"
  run_sudo chmod 0644 "$node"
done

say "Drop-in do TLP (o TLP volta a controlar o perfil)"
if [[ -e /etc/tlp.d/99-alienfan.conf ]]; then
  run_sudo rm -f /etc/tlp.d/99-alienfan.conf
  command -v tlp >/dev/null && run_sudo tlp start
fi

say "Binários"
run_sudo rm -f /usr/local/bin/alienfan /usr/local/bin/alienfand

say "Config e grupo"
if [[ -d $ETC ]] && ask "Apagar $ETC (sua config e curvas)?"; then
  run_sudo rm -r "$ETC"
fi
if getent group "$GROUP" >/dev/null && ask "Remover o grupo $GROUP?"; then
  run_sudo groupdel "$GROUP"
fi

echo
echo "Desinstalado."
