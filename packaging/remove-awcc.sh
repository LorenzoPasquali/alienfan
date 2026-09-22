#!/usr/bin/env bash
# Removes the tr1xem awcc install (SPEC 15), which does not work on this
# machine: "ACPI module not found in kernel", thermal mode 0xffffffff.
#
# It asks before every step and never touches /git/AWCC, the clone that
# alienfan uses as a reference.
set -euo pipefail

FILES=(
  /usr/bin/awcc
  /usr/share/applications/awcc.desktop
  /usr/share/icons/awcc.png
  /etc/udev/rules.d/70-awcc.rules
  /etc/systemd/system/awccd.service
)

say() { printf '\n==> %s\n' "$*"; }
run_sudo() { printf '+ sudo %s\n' "$*"; sudo "$@"; }
ask() { local reply; read -r -p "$1 [s/N] " reply; [[ $reply == [sS]* ]]; }

[[ $EUID -ne 0 ]] || { echo "rode como usuário, sem sudo" >&2; exit 1; }

say "O que existe hoje"
systemctl is-active awccd.service >/dev/null 2>&1 && echo "awccd.service: ativo" || echo "awccd.service: parado"
for f in "${FILES[@]}" /etc/awcc; do
  [[ -e $f ]] && echo "existe: $f"
done

cat <<'EOF'

Antes de remover: o awccd também escutava o teclado (KeyBinder em
/dev/input/event3). Se alguma tecla do teclado dependia dele, ela para de
funcionar. Para conferir, em outro terminal:

  sudo systemctl stop awccd
  sudo libinput debug-events     # aperte a tecla AWCC e as Fn+F*

Se alguma tecla só funciona com o awccd, anote antes de seguir: isso vira
uma questão em aberto (SPEC, seção 18, item 4).
EOF

ask "Remover o awcc agora?" || { echo "nada foi feito"; exit 0; }

say "Serviço"
if systemctl list-unit-files awccd.service >/dev/null 2>&1; then
  run_sudo systemctl disable --now awccd.service
fi

say "Arquivos"
for f in "${FILES[@]}"; do
  [[ -e $f ]] && run_sudo rm -f "$f"
done
if [[ -d /etc/awcc ]] && ask "Apagar /etc/awcc (database.json)?"; then
  run_sudo rm -r /etc/awcc
fi

say "Recarregando systemd e udev"
run_sudo systemctl daemon-reload
run_sudo udevadm control --reload

echo
echo "Pronto. O código-fonte em /git/AWCC ficou intacto."
echo "Confira com: alienfan doctor"
