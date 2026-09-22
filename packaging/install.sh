#!/usr/bin/env bash
# alienfan installer (SPEC 11.4).
#
# Runs as the user and calls sudo only where needed, printing each sudo
# command before it runs. Safe to run again.
#
# Scope so far: CLI, daemon, group, udev rule, config, TLP drop-in, boot
# service and the GNOME extension. The panel (M5) comes later.
set -euo pipefail

REPO=$(cd "$(dirname "$0")/.." && pwd)
BIN=/usr/local/bin
GROUP=alienfan
ETC=/etc/alienfan
UUID=alienfan@lorenzopasquali.github.io
EXT_DIR=$HOME/.local/share/gnome-shell/extensions/$UUID

say() { printf '\n==> %s\n' "$*"; }
die() { printf 'erro: %s\n' "$*" >&2; exit 1; }
run_sudo() { printf '+ sudo %s\n' "$*"; sudo "$@"; }

[[ $EUID -ne 0 ]] || die "rode como usuário, sem sudo: ./packaging/install.sh"
cd "$REPO"

say "1/9 Compilando em release (sem root)"
if ! command -v cargo >/dev/null && [[ -f $HOME/.cargo/env ]]; then
  # shellcheck source=/dev/null
  . "$HOME/.cargo/env"
fi
command -v cargo >/dev/null || die "cargo não encontrado; instale o Rust com rustup"
cargo build --release --locked -p alienfan-cli -p alienfand

say "2/9 Grupo $GROUP"
run_sudo groupadd -f "$GROUP"
if id -nG "$USER" | tr ' ' '\n' | grep -qx "$GROUP"; then
  echo "$USER já está no grupo $GROUP"
else
  run_sudo usermod -aG "$GROUP" "$USER"
fi

say "3/9 Binários em $BIN"
run_sudo install -m 0755 target/release/alienfan target/release/alienfand "$BIN/"

say "4/9 Regra udev (escrita do grupo no perfil e no boost)"
run_sudo install -m 0644 packaging/udev/71-alienfan.rules /etc/udev/rules.d/71-alienfan.rules
run_sudo udevadm control --reload
run_sudo udevadm trigger -c add -s platform-profile --attr-match=name=alienware-wmi
run_sudo udevadm trigger -c add -s hwmon --attr-match=name=alienware_wmi
run_sudo udevadm settle

say "5/9 Config em $ETC"
run_sudo install -d -m 2775 -o root -g "$GROUP" "$ETC"
if [[ -e $ETC/config.toml ]]; then
  echo "$ETC/config.toml já existe; mantido"
else
  run_sudo install -m 0664 -o root -g "$GROUP" packaging/config/config.toml "$ETC/config.toml"
fi

say "6/9 Drop-in do TLP (o TLP deixa de trocar o perfil)"
if command -v tlp >/dev/null; then
  run_sudo install -m 0644 packaging/tlp/99-alienfan.conf /etc/tlp.d/99-alienfan.conf
  run_sudo tlp start
else
  echo "TLP não instalado; pulando"
fi

say "7/9 Serviço de boot (aplica o padrão antes do login)"
run_sudo install -m 0644 packaging/systemd/alienfan-apply.service /etc/systemd/system/alienfan-apply.service
run_sudo systemctl daemon-reload
run_sudo systemctl enable --now alienfan-apply.service

say "8/9 Daemon da sessão (alienfand)"
run_sudo install -m 0644 packaging/systemd/alienfand.service /usr/lib/systemd/user/alienfand.service
run_sudo install -m 0644 packaging/dbus/io.github.lorenzopasquali.AlienFan.service \
  /usr/share/dbus-1/services/io.github.lorenzopasquali.AlienFan.service
systemctl --user daemon-reload
systemctl --user enable alienfand.service
# Restart, not start: a reinstall must pick up the new binary.
systemctl --user restart alienfand.service

say "9/9 Extensão GNOME (sem root)"
mkdir -p "$EXT_DIR"
cp -r "gnome-extension/$UUID/." "$EXT_DIR/"
echo "copiada para $EXT_DIR"

say "Conferindo"
"$BIN/alienfan" doctor || true

cat <<EOF

Instalado. Falta:
  1. Sair da sessão e entrar de novo, para o grupo $GROUP valer.
     (Isso também reinicia o GNOME Shell, que passa a ver a extensão.)
  2. Habilitar a extensão: gnome-extensions enable $UUID
  3. Conferir, sem sudo: alienfan doctor
Numa reinstalação sem sair da sessão: Alt+F2, r, Enter (X11) recarrega a extensão.
Para desfazer: ./packaging/uninstall.sh
EOF
