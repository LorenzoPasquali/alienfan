#!/usr/bin/env bash
# alienfan — Fase 0 (SPEC.md, seção 4): valida perfis e boost no hardware real.
#
# Uso:
#   sudo bash docs/fase0.sh           # T1–T4, T8, T9 (~15 min, na tomada, ociosa)
#   sudo bash docs/fase0.sh prep      # T5/T6: deixa balanced + boost 128 antes de suspender/reiniciar
#   bash docs/fase0.sh st             # mostra o estado atual (não precisa de sudo)
#
# O log vai para docs/fase0-<data>.log. Ctrl+C restaura o estado original.
set -u

P=$(grep -l '^alienware-wmi$' /sys/class/platform-profile/*/name 2>/dev/null | xargs -r dirname)
H=$(grep -l '^alienware_wmi$' /sys/class/hwmon/*/name 2>/dev/null | xargs -r dirname)
[[ -n $P && -n $H ]] || { echo "erro: driver alienware_wmi não encontrado" >&2; exit 1; }

WAIT=${WAIT:-30}
SETTLE=${SETTLE:-60}
DIR=$(cd "$(dirname "$0")" && pwd)

st() {
  echo "profile=$(cat "$P/profile") b1=$(cat "$H/fan1_boost") b2=$(cat "$H/fan2_boost")" \
       "rpm1=$(cat "$H/fan1_input") rpm2=$(cat "$H/fan2_input")" \
       "tcpu=$(cat "$H/temp1_input") tgpu=$(cat "$H/temp2_input")"
}
setp() { echo "$1" > "$P/profile" || echo "  FALHOU: profile=$1"; }
setb() {
  echo "$1" > "$H/fan1_boost" || echo "  FALHOU: fan1_boost=$1"
  echo "$1" > "$H/fan2_boost" || echo "  FALHOU: fan2_boost=$1"
}
need_root() { [[ $EUID -eq 0 ]] || { echo "rode com sudo: sudo bash $0 ${1:-}" >&2; exit 1; }; }

case ${1:-run} in
  st) echo "$(date +%T) $(st)"; exit 0 ;;
  prep)
    need_root prep
    setp balanced; setb 128; sleep 2
    echo "$(date +%T) antes: $(st)"
    echo "Agora suspenda (T5) ou reinicie (T6). Depois rode: bash docs/fase0.sh st"
    exit 0 ;;
  run) need_root ;;
  *) echo "uso: $0 [run|prep|st]" >&2; exit 2 ;;
esac

LOG="$DIR/fase0-$(date +%Y%m%d-%H%M%S).log"
exec > >(tee "$LOG") 2>&1

orig_p=$(cat "$P/profile"); orig_b1=$(cat "$H/fan1_boost"); orig_b2=$(cat "$H/fan2_boost")
restore() {
  setp "$orig_p"
  echo "$orig_b1" > "$H/fan1_boost"; echo "$orig_b2" > "$H/fan2_boost"
  echo "restaurado: $(st)"
}
trap 'echo; echo interrompido; restore; exit 130' INT TERM

echo "# alienfan fase 0 — $(date -Is)"
echo "# kernel=$(uname -r) bios=$(cat /sys/class/dmi/id/bios_version) ac=$(cat /sys/class/power_supply/AC/online)"
echo "# P=$P H=$H WAIT=${WAIT}s SETTLE=${SETTLE}s"
echo "# choices=$(cat "$P/choices")"
systemctl is-active --quiet awccd && echo "# aviso: awccd ativo durante o teste"
[[ $(cat /sys/class/power_supply/AC/online) == 1 ]] || echo "# aviso: fora da tomada"
echo "inicial: $(st)"

# Fans slow down much more slowly than they speed up (T4d of the first run),
# so every step goes up in speed, and a SETTLE pause precedes each descent.
settle() { setb 0; setp quiet; sleep "$SETTLE"; }

echo; echo "## T1: RPM base por perfil, boost 0 (do mais lento ao mais rápido)"
settle
for p in quiet balanced balanced-performance custom cool performance; do
  setp "$p"; sleep "$WAIT"; echo "T1 $p: $(st)"
done

echo; echo "## T2: boost fora do custom (balanced)"
settle; setp balanced
for b in 0 26 51 77 102 128 191 255; do setb "$b"; sleep "$WAIT"; echo "T2 b=$b: $(st)"; done

echo; echo "## T3: boost dentro do custom"
settle; setp custom
for b in 0 128 255; do setb "$b"; sleep "$WAIT"; echo "T3 b=$b: $(st)"; done

echo; echo "## T4: trocar de perfil zera o boost?"
echo 128 > "$H/fan1_boost"; setp balanced; sleep 2
echo "T4a custom->balanced (fan1=128, fan2=255), +2s: $(st)"
setb 128; sleep "$WAIT"; echo "T4b balanced b=128, +${WAIT}s: $(st)"
setp quiet; sleep 2; echo "T4c balanced->quiet, +2s: $(st)"
sleep "$WAIT"; echo "T4d quiet, +${WAIT}s: $(st)"

echo; echo "## T8: reescrever o mesmo perfil zera o boost?"
setp balanced; setb 128; sleep 2; echo "T8a balanced b=128: $(st)"
setp balanced; sleep 2; echo "T8b balanced reescrito, +2s: $(st)"

echo; echo "## T9: no performance, o boost do firmware pode ser trocado?"
settle; setp performance; sleep "$WAIT"; echo "T9a performance: $(st)"
setb 0; sleep "$SETTLE"; echo "T9b performance b=0, +${SETTLE}s: $(st)"
setb 255; sleep "$WAIT"; echo "T9c performance b=255: $(st)"

echo
restore
[[ -n ${SUDO_USER:-} ]] && chown "$SUDO_USER:" "$LOG"
echo "log: $LOG"
echo "Próximo: T5 (suspender) com 'sudo bash docs/fase0.sh prep'."
echo "T6 e T7 saem do log do boot: journalctl -b -u alienfan-apply"
