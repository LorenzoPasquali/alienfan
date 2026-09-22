#!/usr/bin/env bash
# alienfan installer (SPEC 11.4).
#
# Runs as the user and calls sudo only where needed, printing each sudo
# command before it runs. Safe to run again.
#
# Installs the CLI, the daemon, the group, the udev rule, the config, the
# TLP drop-in, the boot service, the GNOME extension and the panel. The
# panel needs the WebKitGTK development packages; without them it is
# skipped with the apt command to run.
set -euo pipefail

REPO=$(cd "$(dirname "$0")/.." && pwd)
BIN=/usr/local/bin
GROUP=alienfan
ETC=/etc/alienfan
UUID=alienfan@lorenzopasquali.github.io
EXT_DIR=$HOME/.local/share/gnome-shell/extensions/$UUID
APP_ID=io.github.lorenzopasquali.AlienFan
SHARE=/usr/local/share
TAURI_DEPS="libwebkit2gtk-4.1-dev libdbus-1-dev libxdo-dev libssl-dev libayatana-appindicator3-dev librsvg2-dev"

say() { printf '\n==> %s\n' "$*"; }
die() { printf 'erro: %s\n' "$*" >&2; exit 1; }
run_sudo() { printf '+ sudo %s\n' "$*"; sudo "$@"; }

[[ $EUID -ne 0 ]] || die "rode como usuário, sem sudo: ./packaging/install.sh"
cd "$REPO"

say "1/10 Compilando em release (sem root)"
if ! command -v cargo >/dev/null && [[ -f $HOME/.cargo/env ]]; then
  # shellcheck source=/dev/null
  . "$HOME/.cargo/env"
fi
command -v cargo >/dev/null || die "cargo não encontrado; instale o Rust com rustup"
cargo build --release --locked -p alienfan-cli -p alienfand

say "2/10 Grupo $GROUP"
run_sudo groupadd -f "$GROUP"
if id -nG "$USER" | tr ' ' '\n' | grep -qx "$GROUP"; then
  echo "$USER já está no grupo $GROUP"
else
  run_sudo usermod -aG "$GROUP" "$USER"
fi

say "3/10 Binários em $BIN"
run_sudo install -m 0755 target/release/alienfan target/release/alienfand "$BIN/"

say "4/10 Regra udev (escrita do grupo no perfil e no boost)"
run_sudo install -m 0644 packaging/udev/71-alienfan.rules /etc/udev/rules.d/71-alienfan.rules
run_sudo udevadm control --reload
run_sudo udevadm trigger -c add -s platform-profile --attr-match=name=alienware-wmi
run_sudo udevadm trigger -c add -s hwmon --attr-match=name=alienware_wmi
run_sudo udevadm settle

say "5/10 Config em $ETC"
run_sudo install -d -m 2775 -o root -g "$GROUP" "$ETC"
if [[ -e $ETC/config.toml ]]; then
  echo "$ETC/config.toml já existe; mantido"
else
  run_sudo install -m 0664 -o root -g "$GROUP" packaging/config/config.toml "$ETC/config.toml"
fi

say "6/10 Drop-in do TLP (o TLP deixa de trocar o perfil)"
if command -v tlp >/dev/null; then
  run_sudo install -m 0644 packaging/tlp/99-alienfan.conf /etc/tlp.d/99-alienfan.conf
  run_sudo tlp start
else
  echo "TLP não instalado; pulando"
fi

say "7/10 Serviço de boot (aplica o padrão antes do login)"
run_sudo install -m 0644 packaging/systemd/alienfan-apply.service /etc/systemd/system/alienfan-apply.service
run_sudo systemctl daemon-reload
run_sudo systemctl enable --now alienfan-apply.service

say "8/10 Daemon da sessão (alienfand)"
run_sudo install -m 0644 packaging/systemd/alienfand.service /usr/lib/systemd/user/alienfand.service
run_sudo install -m 0644 packaging/dbus/io.github.lorenzopasquali.AlienFan.service \
  /usr/share/dbus-1/services/io.github.lorenzopasquali.AlienFan.service
systemctl --user daemon-reload
systemctl --user enable alienfand.service
# Restart, not start: a reinstall must pick up the new binary.
systemctl --user restart alienfand.service

say "9/10 Extensão GNOME (sem root)"
mkdir -p "$EXT_DIR"
cp -r "gnome-extension/$UUID/." "$EXT_DIR/"
echo "copiada para $EXT_DIR"

say "10/10 Painel (alienfan-panel)"
if ! pkg-config --exists webkit2gtk-4.1 dbus-1; then
  echo "Painel pulado: faltam os pacotes de desenvolvimento do WebKitGTK."
  echo "Instale e rode este script de novo:"
  echo "  sudo apt install $TAURI_DEPS"
elif ! command -v npm >/dev/null; then
  echo "Painel pulado: npm não encontrado."
else
  (cd panel && npm ci --no-audit --no-fund && npx tauri build --no-bundle)
  run_sudo install -m 0755 panel/src-tauri/target/release/alienfan-panel "$BIN/alienfan-panel"
  run_sudo install -D -m 0644 "packaging/desktop/$APP_ID.desktop" "$SHARE/applications/$APP_ID.desktop"
  run_sudo install -D -m 0644 "packaging/icons/$APP_ID.svg" "$SHARE/icons/hicolor/scalable/apps/$APP_ID.svg"
  run_sudo install -D -m 0644 "packaging/icons/$APP_ID.png" "$SHARE/icons/hicolor/256x256/apps/$APP_ID.png"
  if command -v gtk-update-icon-cache >/dev/null; then
    run_sudo gtk-update-icon-cache -q -t -f "$SHARE/icons/hicolor"
  fi
fi

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
