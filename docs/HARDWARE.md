# Hardware validation (Phase 0)

Results of `docs/fase0.sh` on the Alienware 16 Aurora AC16250, BIOS 1.15.0,
kernel 7.0.0-31-generic. Raw logs are next to this file (`fase0-*.log`).

## Run 1: 2026-09-22, on battery

Log: `fase0-20260922-115939.log`. The spec asked for AC power; this run was on
battery (`ac=0`) with `awccd` active, and waited 20 s per step.

### T1: base RPM per profile, boost 0

| Profile | CPU rpm | GPU rpm | `fanN_boost` read back |
|---|---|---|---|
| `quiet` | 0 | 0 | 0 |
| `cool` | 4285 | 4398 | 0 |
| `balanced` | 1766* | 1732* | 0 |
| `balanced-performance` | 2391 | 2540 | 0 |
| `performance` | 6204 | 6166 | **100** (set by the firmware/driver) |
| `custom` | 2861* | 2944* | 0 |

\* Measured right after a faster step. Fans slow down slowly (see T4d), so
these are upper bounds.

### T2 / T3: boost in `balanced` and in `custom`

| Boost | `balanced` CPU / GPU rpm | `custom` CPU / GPU rpm |
|---|---|---|
| 0 | 1768 / 1746 | 2992 / 3213* |
| 128 (50%) | 5802 / 5940 | 6622 / 6557 |
| 255 (100%) | 6600 / 6622 | 6600 / 6688 |

Extra point from the M2 acceptance test (same day, on battery): `balanced`
with boost 51 (20%) settles at 3276 / 3260 rpm.

### T4: profile change

- `custom` → `balanced` with boost 128/255: both read back **0** after 2 s.
- `balanced` → `quiet` with boost 128: both read back **0** after 2 s.
- RPM falls slowly: 6300 → 2155 in 20 s after switching to `quiet`, while
  `quiet` idles at 0.

## Decisions

1. **Boost works outside `custom`.** `hardware.boost_requires_custom = false`
   (already the shipped value).
2. **A profile change resets the boost to the firmware value.** Every plan
   writes the boost after the profile, forced (`Write::Boost { force: true }`),
   as SPEC 6.4 planned.
3. **The GPU fan follows the boost with the GPU idle.** The UI note "GPU fan
   stopped" only applies when rpm and boost are both 0.
4. **The boost response is steep.** 50% boost already gives ~90% of the top
   speed, so curves need small boost values in the 0–30% range.
5. **Real top speed is ~6600 rpm, above `fanN_max` = 6000.** The UI clamps
   the RPM ring at 100%.
6. **`custom` idles faster than `balanced`** (~2900 vs ~1750 rpm, upper
   bounds). With `boost_requires_custom = false` the daemon never needs it
   (SPEC 18, question 2).

## Open

- **`performance` sets boost 100 on its own.** Today every write plan also
  writes the boost, so `alienfan profile set performance` keeps the previous
  boost (usually 0) instead of the firmware's 100. Run 2 (T9) shows whether
  writing 0 there lowers the fans. Then decide what "Automático (firmware)"
  means in `performance`.
- **T8**: does writing the same profile again reset the boost? This would give
  a way back to the firmware value without a hardcoded table.
- **Run 2 on AC**: base RPM per profile on AC, with ascending steps and a
  settle pause before each descent (`sudo bash docs/fase0.sh`, ~15 min).
- **T5** (suspend/resume keeps profile and boost?): `sudo bash docs/fase0.sh
  prep`, suspend, resume, `bash docs/fase0.sh st`.
- **T6/T7** (what the firmware boots with): `alienfan apply --boot` now logs
  the state it found. After a reboot: `journalctl -b -u alienfan-apply`.
