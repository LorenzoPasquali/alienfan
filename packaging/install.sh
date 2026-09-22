#!/usr/bin/env bash
# alienfan installer (SPEC 11.4).
#
# Runs as the user and calls sudo only where needed, printing each sudo
# command before it runs. Safe to run again.
#
# M2 scope: CLI, group, udev rule, config, TLP drop-in and boot service.
# The daemon (M3), the GNOME extension (M4) and the panel (M5) come later.
set -euo pipefail

REPO=$(cd "$(dirname "$0")/.." && pwd)
BIN=/usr/local/bin
GROUP=alienfan
ETC=/etc/alienfan

say() { printf '\n==> %s\n' "$*"; }
die() { printf 'erro: %s\n' "$*" >&2; exit 1; }
run_sudo() { printf '+ sudo %s\n' "$*"; sudo "$@"; }

[[ $EUID -ne 0 ]] || die "rode como usuário, sem sudo: ./packaging/install.sh"
cd "$REPO"

say "1/7 Compilando em release (sem root)"
if ! command -v cargo >/dev/null && [[ -f $HOME/.cargo/env ]]; then
  # shellcheck source=/dev/null
  . "$HOME/.cargo/env"
fi
command -v cargo >/dev/null || die "cargo não encontrado; instale o Rust com rustup"
cargo build --release --locked -p alienfan-cli

say "2/7 Grupo $GROUP"
run_sudo groupadd -f "$GROUP"
if id -nG "$USER" | tr ' ' '\n' | grep -qx "$GROUP"; then
  echo "$USER já está no grupo $GROUP"
else
  run_sudo usermod -aG "$GROUP" "$USER"
fi

say "3/7 Binário em $BIN"
run_sudo install -m 0755 target/release/alienfan "$BIN/alienfan"

say "4/7 Regra udev (escrita do grupo no perfil e no boost)"
run_sudo install -m 0644 packaging/udev/71-alienfan.rules /etc/udev/rules.d/71-alienfan.rules
run_sudo udevadm control --reload
run_sudo udevadm trigger -c add -s platform-profile --attr-match=name=alienware-wmi
run_sudo udevadm trigger -c add -s hwmon --attr-match=name=alienware_wmi
run_sudo udevadm settle

say "5/7 Config em $ETC"
run_sudo install -d -m 2775 -o root -g "$GROUP" "$ETC"
if [[ -e $ETC/config.toml ]]; then
  echo "$ETC/config.toml já existe; mantido"
else
  run_sudo install -m 0664 -o root -g "$GROUP" packaging/config/config.toml "$ETC/config.toml"
fi

say "6/7 Drop-in do TLP (o TLP deixa de trocar o perfil)"
if command -v tlp >/dev/null; then
  run_sudo install -m 0644 packaging/tlp/99-alienfan.conf /etc/tlp.d/99-alienfan.conf
  run_sudo tlp start
else
  echo "TLP não instalado; pulando"
fi

say "7/7 Serviço de boot (aplica o padrão antes do login)"
run_sudo install -m 0644 packaging/systemd/alienfan-apply.service /etc/systemd/system/alienfan-apply.service
run_sudo systemctl daemon-reload
run_sudo systemctl enable --now alienfan-apply.service

say "Conferindo"
"$BIN/alienfan" doctor || true

cat <<EOF

Instalado. Falta:
  1. Sair da sessão e entrar de novo, para o grupo $GROUP valer.
  2. Depois, sem sudo: alienfan doctor && alienfan profile set quiet
Para desfazer: ./packaging/uninstall.sh
EOF
