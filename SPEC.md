# alienfan — Especificação

Controle de perfil térmico e ventoinhas do Alienware 16 Aurora no Zorin OS, com
botão nas Configurações Rápidas do GNOME, painel gráfico, curva de ventoinha e
padrão persistente para tomada e bateria.

- **Status:** especificação, nada implementado.
- **Data:** 2026-09-21.
- **Pasta do projeto:** `/git/alienfan`.
- **Público:** o agente que vai refinar e implementar esta spec. Leia tudo antes
  de escrever código. As seções 3 e 4 mudam decisões de implementação.

---

## 1. Objetivo

Hoje, para as ventoinhas funcionarem bem, o usuário inicia o Windows, ajusta o
Alienware Command Center (AWCC) e volta ao Linux. O ajuste não vale no Linux,
porque o AWCC aplica o modo de novo a cada boot do Windows e não grava nada no
hardware. No Zorin o notebook sobe em `balanced`.

O alienfan deve:

1. Trocar o perfil térmico pelo menu do GNOME, sem terminal e sem senha.
2. Acelerar as ventoinhas manualmente (boost por ventoinha).
3. Seguir uma curva temperatura → boost, se o usuário quiser.
4. Guardar um padrão para a tomada e outro para a bateria, e aplicar cada um
   sozinho no boot e quando a fonte de energia mudar.
5. Mostrar RPM e temperatura ao vivo em um painel grande, com ventoinhas animadas.
6. Substituir o `awcc` (tr1xem) instalado, que não funciona nesta máquina.

---

## 2. Ambiente verificado (2026-09-21)

Tudo nesta seção foi lido na máquina. O que não foi verificado está marcado.

### 2.1 Sistema

| Item | Valor |
|---|---|
| Modelo | Alienware 16 Aurora AC16250 (placa 0NPPY1) |
| BIOS | 1.15.0 (07/08/2026) |
| SO | Zorin OS 18.1, base Ubuntu 24.04 `noble` |
| Kernel | `7.0.0-31-generic` |
| Desktop | GNOME Shell 46.0, sessão **X11** (`XDG_CURRENT_DESKTOP=zorin:GNOME`) |
| Usuário | `lorenzopasquali` (uid 1000), está no grupo `sudo`, sem sudo sem senha |
| Rust | **não instalado** (sem `cargo`, `rustc` e `rustup`) |
| Node | `v24.21.0`, npm `11.19.0` |
| git | `2.43.0` |
| Pacotes disponíveis no apt | `libwebkit2gtk-4.1-dev` 2.52.6, `libgtk-4-dev` 4.14.5, `libadwaita-1-dev` 1.5.0 |

### 2.2 Drivers e sysfs

Módulos carregados: `alienware_wmi`, `dell_wmi`, `dell_smbios`, `dell_smm_hwmon`,
`dell_wmi_ddv`, `dell_wmi_sysman`, `platform_profile`.

**Números `hwmonN` mudam entre boots.** Sempre encontre o dispositivo pelo arquivo
`name`. Os números abaixo são só da sessão onde a leitura foi feita.

#### Perfil térmico (driver `alienware_wmi`)

- Class device: `/sys/class/platform-profile/platform-profile-0/`
  - `name` = `alienware-wmi`
  - `choices` = `cool quiet balanced balanced-performance performance custom`
  - `profile`: RW, `root:root 0644`
  - udev: `SUBSYSTEM=platform-profile`, `ATTR{name}=="alienware-wmi"`
- Interface legada: `/sys/firmware/acpi/platform_profile` e `platform_profile_choices`.
  Ela escreve em todos os handlers. **Não use.** Use o class device.
- Segundo a doc do kernel, `performance` também liga o **G-Mode** quando o
  modelo suporta.

#### Ventoinhas e sensores AWCC (hwmon `name` = `alienware_wmi`, era `hwmon7`)

| Arquivo | Valor lido | Obs. |
|---|---|---|
| `fan1_label` | `CPU Fan` | |
| `fan2_label` | `GPU Fan` | |
| `fan1_input`, `fan2_input` | RPM (ex.: 1718, 0) | a GPU fica em 0 quando ociosa |
| `fan1_min/max`, `fan2_min/max` | 0 / 6000 | |
| `fan1_boost`, `fan2_boost` | 0 | **RW, 0–255**, `root:root 0644` |
| `temp1_label` / `temp1_input` | `CPU` / 46000 | m°C |
| `temp2_label` / `temp2_input` | `GPU` / 38000 | m°C |
| `pwm1_auto_channels_temp`, `pwm2_auto_channels_temp` | 1, 2 | ventoinha N segue o sensor N |

- Caminho do device:
  `/sys/devices/platform/PNP0C14:0a/wmi_bus/wmi_bus-PNP0C14:0a/A70591CE-A997-11DA-B012-B622A1EF5492/hwmon/hwmonN`
- udev: `SUBSYSTEM=="hwmon"`, `ATTR{name}=="alienware_wmi"`.

Comportamento do boost segundo a doc do kernel
(`Documentation/admin-guide/laptops/alienware-wmi.rst`):

```
pwm = pwm_base + (fan_boost / 255) * (pwm_max - pwm_base)
```

> "In some devices, manual fan control only works reliably if the `custom`
> platform profile is selected."

#### Outros sensores (só leitura)

- hwmon `dell_ddv` (era `hwmon8`): `fan1` "CPU Fan", `fan2` "Video Fan", e
  `temp1..9` com labels `CPU, CPU, Other, Other, HDD, HDD, Memory, Unknown, Video`.
  Os labels se repetem, então identifique o sensor pelo índice.
- hwmon `coretemp` (era `hwmon5`): `Package id 0` e núcleos.
- hwmon `dell_smm` (era `hwmon9`): `pwm1`/`pwm2`, `pwm1_enable`=1, `fan1_target`.
  **Nunca escreva aqui.** Ele pode brigar com o controle AWCC/EC. Pode ser usado
  só para leitura, como fallback de RPM.

#### Energia

- `/sys/class/power_supply/AC/online`: 0 ou 1, `POWER_SUPPLY_TYPE=Mains`.
- UPower está rodando e expõe `org.freedesktop.UPower` com a propriedade
  `OnBattery` no bus de sistema.

### 2.3 Serviços que interferem

| Serviço | Estado | Impacto |
|---|---|---|
| `tlp.service` 1.6.1 | ativo | Define o perfil térmico no boot e na troca tomada/bateria. Os padrões do TLP (comentados em `/etc/tlp.conf`, linhas 208–209) são `PLATFORM_PROFILE_ON_AC=performance` e `PLATFORM_PROFILE_ON_BAT=low-power`. `low-power` não existe neste modelo, então na bateria o perfil fica `balanced`. |
| `thermald` (`--adaptive`) | ativo | Faz throttling de CPU, não mexe em ventoinha. Deixe como está. |
| `awccd.service` (tr1xem AWCC v1.12.0) | ativo | Quebrado aqui. Log: `ACPI module not found in kernel`, `Unknown thermal mode returned: 0xffffffff`, `LightFX ... Failed to find device`. Também escuta o teclado em `/dev/input/event3` (KeyBinder). Será removido (seção 15). |
| `power-profiles-daemon` | ausente | Sem conflito. Por isso o GNOME não mostra "Modo de energia" nas Configurações Rápidas. |

**Como desligar o perfil no TLP** (verificado em
`/usr/share/tlp/func.d/10-tlp-func-cpu`, por volta da linha 507): se
`PLATFORM_PROFILE_ON_AC` ou `PLATFORM_PROFILE_ON_BAT` estiver vazio, o TLP faz
`return 0` e não mexe no perfil. Um drop-in com os dois vazios basta.

### 2.4 Lição do incidente do Dash to Dock (2026-09-21)

O Dash to Dock v108 declarou suporte ao GNOME 46 mas chama `Clutter.ClickGesture`,
que só existe no GNOME 49+. A barra do usuário sumiu. Regra para a extensão deste
projeto: **só use APIs do GNOME 46.** Detecte recursos novos antes de usar, e
declare em `shell-version` só as versões que foram testadas.

---

## 3. Limites do hardware e decisões que eles impõem

1. **O boost só acelera.** Ele soma em cima da base do firmware. Não existe como
   deixar a ventoinha abaixo da base da BIOS por este driver.
   - Para "deixar mais lento", a UI troca para o perfil `quiet` ou `cool` com
     boost 0. A UI deve explicar isso ao usuário.
2. **A falha é segura por natureza.** Se o daemon morrer, o pior caso é boost 0,
   que é o comportamento normal do firmware. O firmware continua protegendo o
   hardware. Não existe estado em que o alienfan deixe a ventoinha parada numa
   CPU quente.
3. O **boost pode exigir o perfil `custom`**. Isso só se descobre na Fase 0. A
   config guarda o resultado em `hardware.boost_requires_custom` e o daemon se
   adapta (seção 6.4).
4. **`performance` liga o G-Mode**, que é barulhento. O usuário já disse que
   `performance` "é demais". O padrão sugerido na tomada é `balanced-performance`.
5. **Não toque no `dell_smm`**, nem no `pwm*` nem no `pwm*_enable`.

---

## 4. Fase 0 — Validação do hardware (obrigatória, antes do código)

O usuário roda estes comandos (precisam de sudo). O agente anota os resultados
em `docs/HARDWARE.md`. Espere pelo menos 20 s depois de cada mudança antes de
ler o RPM. Faça os testes na tomada, com a máquina ociosa.

```bash
P=$(grep -l alienware-wmi /sys/class/platform-profile/*/name | xargs dirname)
H=$(grep -l '^alienware_wmi$' /sys/class/hwmon/*/name | xargs dirname)
st(){ echo "profile=$(cat $P/profile) b1=$(cat $H/fan1_boost) b2=$(cat $H/fan2_boost) rpm1=$(cat $H/fan1_input) rpm2=$(cat $H/fan2_input) tcpu=$(cat $H/temp1_input) tgpu=$(cat $H/temp2_input)"; }

# T1: RPM base em cada perfil, com boost 0
for p in quiet cool balanced balanced-performance performance custom; do
  echo $p | sudo tee $P/profile >/dev/null; sleep 20; st; done

# T2: o boost funciona fora do custom?
echo balanced | sudo tee $P/profile; for b in 0 128 255; do
  echo $b | sudo tee $H/fan1_boost $H/fan2_boost >/dev/null; sleep 20; st; done

# T3: e dentro do custom?
echo custom | sudo tee $P/profile; for b in 0 128 255; do
  echo $b | sudo tee $H/fan1_boost $H/fan2_boost >/dev/null; sleep 20; st; done

# T4: trocar o perfil zera o boost?
echo 128 | sudo tee $H/fan1_boost; echo balanced | sudo tee $P/profile; sleep 2; st

# T5: suspender e voltar mantém perfil e boost?  (suspender, voltar, rodar st)
# T6: reboot mantém perfil e boost?  (esperado: não)
# T7: qual perfil o firmware usa no boot?  (st logo após login, com o TLP já neutralizado)
```

Decisões que saem daqui:

| Pergunta | Afeta |
|---|---|
| O boost só funciona em `custom`? | `hardware.boost_requires_custom` |
| Trocar de perfil zera o boost? | O daemon reaplica o boost depois de cada `SetProfile` |
| Suspender reseta o estado? | Reaplicar depois do resume (planejado de qualquer jeito) |
| Qual a base de RPM de cada perfil? | Textos de ajuda da UI e presets |
| O `fan2` (GPU) responde ao boost com a GPU ociosa? | Mensagem na UI quando o RPM for 0 |

Se o boost não funcionar em nenhum perfil, a curva e o controle manual saem do
escopo. O projeto vira só um seletor de perfis com padrão persistente. Nesse
caso, avise o usuário antes de continuar.

---

## 5. Arquitetura

```
┌───────────────────────────────┐
│ Extensão GNOME (GJS, Shell 46)│──┐
│ QuickMenuToggle "Ventoinhas"  │  │
└───────────────────────────────┘  │
┌───────────────────────────────┐  │  D-Bus de sessão
│ alienfan-panel (Tauri 2)      │──┼──────────────────┐
│ UI HTML/CSS/SVG + Rust (zbus) │  │                  ▼
└───────────────────────────────┘  │   ┌────────────────────────────────┐
┌───────────────────────────────┐  │   │ alienfand (Rust, systemd --user)│
│ alienfan (CLI)                │──┘   │ estado, curva, override,        │
└───────────────────────────────┘      │ UPower (bateria), logind (resume)│
                                       └──────────────┬─────────────────┘
                                                      │ escrita direta
                                                      ▼
            /sys/class/platform-profile/<alienware-wmi>/profile
            /sys/class/hwmon/<alienware_wmi>/fan{1,2}_boost
            (regra udev: root:alienfan, g+w)

Boot (antes do login):  alienfan-apply.service (sistema, oneshot) → `alienfan apply --boot`
Configuração:           /etc/alienfan/config.toml (root:alienfan 0664)
TLP:                    /etc/tlp.d/99-alienfan.conf  →  PLATFORM_PROFILE_ON_AC="" / _BAT=""
```

Princípios:

- **Existe um único dono do estado em tempo de execução: o `alienfand`.** Extensão,
  painel e CLI só enviam pedidos a ele.
- **O daemon roda como usuário.** O acesso ao hardware vem da regra udev e do
  grupo `alienfan`. Nenhum processo root fica rodando o tempo todo.
- **O boot não depende do login.** O `alienfan-apply.service` aplica o perfil e o
  boost fixo. A curva só começa quando o daemon sobe, no login.
- **O daemon não é obrigatório para a CLI.** Com o daemon parado, a CLI escreve
  direto no sysfs.

### 5.1 Estrutura do repositório

```
/git/alienfan/
├── SPEC.md                      # este arquivo
├── README.md
├── Cargo.toml                   # workspace
├── rust-toolchain.toml          # stable
├── crates/
│   ├── alienfan-core/           # sysfs, modelo, config, curva, validação (sem D-Bus)
│   ├── alienfan-proto/          # nomes D-Bus, XML, proxy zbus, tipos compartilhados
│   ├── alienfand/               # binário do daemon
│   └── alienfan-cli/            # binário `alienfan`
├── panel/                       # app Tauri 2
│   ├── src-tauri/               # crate Rust do painel (usa alienfan-proto)
│   ├── src/                     # UI em TypeScript, sem framework
│   ├── index.html
│   └── package.json
├── gnome-extension/
│   └── alienfan@lorenzopasquali.github.io/
│       ├── metadata.json
│       ├── extension.js
│       ├── dbus.js              # XML da interface e wrapper do proxy
│       ├── stylesheet.css
│       └── icons/alienfan-symbolic.svg
├── packaging/
│   ├── udev/71-alienfan.rules
│   ├── systemd/alienfan-apply.service
│   ├── systemd/alienfand.service                          # unit de usuário
│   ├── dbus/io.github.lorenzopasquali.AlienFan.service    # ativação D-Bus
│   ├── tlp/99-alienfan.conf
│   ├── config/config.toml                                  # config padrão
│   ├── desktop/io.github.lorenzopasquali.AlienFan.desktop
│   ├── icons/                                              # ícones do app
│   ├── install.sh
│   └── uninstall.sh
├── docs/
│   └── HARDWARE.md              # resultados da Fase 0
└── tests/
    └── fixtures/fake-sysfs/     # árvore sysfs falsa para testes
```

### 5.2 Nomes

| Item | Valor |
|---|---|
| Bus name D-Bus | `io.github.lorenzopasquali.AlienFan` |
| Object path | `/io/github/lorenzopasquali/AlienFan` |
| Interface | `io.github.lorenzopasquali.AlienFan1` |
| App ID (Tauri/desktop) | `io.github.lorenzopasquali.AlienFan` |
| UUID da extensão | `alienfan@lorenzopasquali.github.io` |
| Grupo Unix | `alienfan` |
| Binários | `/usr/local/bin/alienfan`, `/usr/local/bin/alienfand`, `/usr/local/bin/alienfan-panel` |

### 5.3 Dependências (Rust)

Use a última versão estável de cada crate no momento da implementação e fixe no
`Cargo.lock`. Sugestões: `zbus` (async, tokio), `tokio`, `serde` + `toml`,
`clap` (derive), `thiserror`, `anyhow` (só nos binários), `tracing` +
`tracing-journald`, `tauri` 2 + `tauri-plugin-single-instance`. Evite crates
grandes sem motivo.

---

## 6. Modelo de domínio (`alienfan-core`)

### 6.1 Tipos

```rust
enum Profile { Cool, Quiet, Balanced, BalancedPerformance, Performance, Custom }
// A string do sysfs é a fonte da verdade (Display/FromStr com os nomes exatos do kernel).
// A lista disponível vem sempre de `choices`, nunca de uma lista fixa no código.

enum FanId { Cpu, Gpu }          // fan1 = Cpu, fan2 = Gpu (confirmar pelo label)

struct Boost(u8);                // 0..=255 (unidade do hardware)
// UI e CLI mostram porcentagem: pct = round(boost * 100 / 255)
// e aceitam: boost = round(pct * 255 / 100).

enum PowerSource { Ac, Battery }

enum Control {
    Firmware,                          // boost 0 nas duas ventoinhas
    Fixed { cpu: Boost, gpu: Boost },
    Curve { name: String },            // referência a [curves.<name>]
}

struct Preset { profile: Profile, control: Control }

struct CurvePoint { temp_c: f32, boost: Boost }
struct Curve {
    cpu: Vec<CurvePoint>,
    gpu: Vec<CurvePoint>,
    hysteresis_c: f32,          // padrão 3.0
    ramp_up_per_s: u16,         // padrão 40 (unidades de boost por segundo)
    ramp_down_per_s: u16,       // padrão 10
}

enum Health { Ok, Degraded, Emergency, NoPermission, NoDriver }
```

### 6.2 Validação (erros com mensagem em pt-BR)

- O perfil precisa estar no `choices` atual.
- O boost vai de 0 a 255. Na CLI, porcentagem vai de 0 a 100.
- Cada curva tem de 2 a 16 pontos.
  - `temp_c` fica entre 0 e 105 e é **estritamente crescente**.
  - `boost` é **não decrescente**. Uma curva que baixa a ventoinha quando esquenta
    é rejeitada.
- `hysteresis_c` fica entre 0 e 15. As rampas vão de 1 a 255.
- Todo `Control::Curve.name` precisa existir em `[curves]`.

### 6.3 Interpolação da curva

- Abaixo do primeiro ponto, vale o boost do primeiro ponto.
- Acima do último, vale o boost do último.
- Entre dois pontos, interpolação linear, arredondada.

### 6.4 Aplicar um preset

Função pura que recebe o estado desejado e devolve uma lista ordenada de escritas:

1. Calcule o perfil efetivo. Se `boost_requires_custom` for `true` e o controle
   não for `Firmware`, o perfil efetivo é `custom`, qualquer que seja o do preset.
   A UI mostra o perfil do preset como "substituído por custom".
2. Escreva o perfil, só se for diferente do atual.
3. Escreva o boost de cada ventoinha. **Sempre depois do perfil**, porque trocar
   de perfil pode zerar o boost (ver T4).

### 6.5 Acesso ao sysfs

- `SysfsRoot` é configurável: o padrão é `/`, e a variável `ALIENFAN_SYSFS_ROOT`
  troca a raiz nos testes.
- Descoberta:
  - O perfil é o `platform-profile-*` cujo `name == "alienware-wmi"`.
  - O hwmon é o `hwmon*` cujo `name == "alienware_wmi"`.
  - A tomada é o `power_supply/*` com `type == "Mains"`.
  - Faça a descoberta de novo a cada falha de I/O, porque o módulo pode ser
    recarregado.
- Leitura: o arquivo inteiro, com `trim`. Temperatura em m°C vira `f32` °C.
- Escrita: `write` direto do valor. Trate `EACCES` como `NoPermission`,
  `ENOENT`/`ENODEV` como `NoDriver`, e `EINVAL` como erro de validação.
- **Faça escrita idempotente:** não escreva se o valor lido já é o desejado. A
  chamada WMI tem custo.

---

## 7. Configuração

### 7.1 Arquivo

- Caminho: `/etc/alienfan/config.toml`.
- Pasta `/etc/alienfan`: `root:alienfan 2775`. Arquivo: `root:alienfan 0664`.
- Quem lê: o `alienfan apply --boot` (root) e o daemon (usuário).
- Quem escreve: o daemon, quando o usuário salva um padrão.
- **Escrita atômica:** grave `config.toml.tmp` na mesma pasta, faça `fsync` e
  `rename`. Antes, copie o anterior para `config.toml.bak`.
- Preserve os comentários quando der. Use `toml_edit` se o custo for baixo. Se
  não der, reescreva o arquivo em formato canônico com um cabeçalho explicativo.
- Se o arquivo não existir, use os padrões embutidos, iguais ao exemplo abaixo.
- Se o arquivo for inválido:
  - O daemon sobe com os padrões embutidos, `Health=Degraded`, e loga o erro
    com linha e coluna.
  - O boot não faz nada e sai com código 0. **Nunca bloqueie o boot.**

### 7.2 Esquema e exemplo (é o `packaging/config/config.toml`)

```toml
# alienfan — configuração. Editável à mão ou pelo painel.
version = 1

[hardware]
boost_requires_custom = false   # preenchido a partir da Fase 0 (docs/HARDWARE.md)
emergency_temp_c = 95.0         # a partir daqui: boost 255 nas duas ventoinhas

[daemon]
tick_ms = 1000                  # período do loop de telemetria e curva (250..=5000)
override_until = "power-change" # "power-change" | "manual"

[sensors]
cpu = "alienware_wmi:CPU"       # "<hwmon name>:<label>" ou "<hwmon name>#<índice>"
gpu = "alienware_wmi:GPU"

[defaults.ac]
profile = "balanced-performance"
control = "firmware"            # "firmware" | "fixed" | "curve"
fixed = { cpu = 0, gpu = 0 }    # usado quando control = "fixed"
curve = "equilibrado"           # usado quando control = "curve"

[defaults.battery]
profile = "balanced"
control = "firmware"
fixed = { cpu = 0, gpu = 0 }
curve = "silencioso"

# Pontos: [temperatura °C, boost 0–255]
[curves.silencioso]
cpu = [[50, 0], [70, 38], [80, 102], [90, 204], [95, 255]]
gpu = [[50, 0], [70, 38], [80, 102], [90, 204], [95, 255]]
hysteresis_c = 3.0
ramp_up_per_s = 40
ramp_down_per_s = 10

[curves.equilibrado]
cpu = [[45, 0], [60, 51], [70, 115], [80, 179], [90, 255]]
gpu = [[45, 0], [60, 51], [70, 115], [80, 179], [90, 255]]
hysteresis_c = 3.0
ramp_up_per_s = 40
ramp_down_per_s = 10

[curves.agressivo]
cpu = [[40, 26], [55, 102], [65, 166], [75, 217], [85, 255]]
gpu = [[40, 26], [55, 102], [65, 166], [75, 217], [85, 255]]
hysteresis_c = 2.0
ramp_up_per_s = 80
ramp_down_per_s = 15
```

Nota sobre o sensor da CPU: `alienware_wmi:CPU` é o sensor que o próprio EC usa
para a fan1 (`pwm1_auto_channels_temp=1`). O `coretemp:Package id 0` reage mais
rápido e dá picos maiores. Mantenha o do EC como padrão e permita trocar.

---

## 8. Daemon `alienfand`

### 8.1 Ciclo de vida

- Unit de usuário `alienfand.service`: `Type=dbus`,
  `BusName=io.github.lorenzopasquali.AlienFan`, `Restart=on-failure`,
  `RestartSec=2`, `WantedBy=default.target`.
- Também sobe por ativação D-Bus (arquivo em `/usr/share/dbus-1/services/`, com
  `SystemdService=alienfand.service`).
- Na partida:
  1. Carregue a config.
  2. Descubra o hardware.
  3. Leia a fonte de energia.
  4. Aplique o preset padrão da fonte atual.
  5. Publique no D-Bus.
- No encerramento (SIGTERM/SIGINT) com controle `Curve` ou `Fixed`, escreva
  boost 0 nas duas ventoinhas. O perfil fica como está. Loge o que fez.
- Uma única instância por sessão. Se o bus name já estiver ocupado, saia com erro.

### 8.2 Estado em tempo de execução

```
effective = override.unwrap_or(defaults[power_source])
```

- **Override:** qualquer mudança manual (`SetProfile`, `SetFixedBoost`, `SetControl`,
  `SetCurve` com aplicação) cria ou atualiza um override na memória.
  `OverrideActive = true`.
- O override acaba quando:
  - o usuário chama `RestoreDefault`;
  - a fonte de energia muda, se `override_until = "power-change"`;
  - o daemon reinicia.
- `SaveAsDefault(target)` copia o `effective` para `defaults.<target>`, grava a
  config e limpa o override.

### 8.3 Eventos

| Evento | Fonte | Ação |
|---|---|---|
| Troca tomada/bateria | UPower, `PropertiesChanged` de `OnBattery` (bus de sistema). Fallback: ler `AC/online` a cada tick | Atualiza `PowerSource`. Aplica o padrão da nova fonte, ou mantém o override se `override_until = "manual"` |
| Voltou da suspensão | logind, sinal `PrepareForSleep(false)` (bus de sistema) | Espera 2 s, redescobre o hardware e reaplica tudo |
| Config mudou no disco | inotify em `/etc/alienfan/config.toml` | Recarrega. Se for inválida, mantém a anterior e marca `Degraded` |
| Tick (`tick_ms`) | timer | Lê a telemetria, roda a curva se o controle for `Curve`, checa emergência, emite `Telemetry` |

### 8.4 Algoritmo da curva (por ventoinha, a cada tick)

```
t_raw   = ler sensor configurado
t       = ema(t_raw, alpha = 0.35)                    # suaviza leitura ruidosa
alvo    = interpolar(curva, t)
se alvo > atual:
    novo = min(alvo, atual + ramp_up_per_s * dt)
    t_ultima_subida = t
senão se alvo < atual e t <= t_ultima_subida - hysteresis_c:
    novo = max(alvo, atual - ramp_down_per_s * dt)
senão:
    novo = atual
se |novo - ultimo_escrito| >= 2 ou novo em {0, 255}: escrever
```

Segurança:

- **Emergência:** se qualquer sensor da CPU ou GPU passar de `emergency_temp_c`,
  escreva boost 255 nas duas ventoinhas, em qualquer controle, e marque
  `Health=Emergency`. Saia do modo quando a temperatura ficar 5 °C abaixo do
  limite por 10 s seguidos. Então volte ao controle anterior.
- **Falha de sensor:** com 3 leituras seguidas falhando, escreva boost 0 (o
  firmware reassume) e marque `Health=Degraded`. Tente de novo a cada tick.
- **Sem permissão:** com `EACCES`, marque `Health=NoPermission`, pare de escrever
  e continue emitindo telemetria. A UI mostra como corrigir.

### 8.5 Logs

Use `tracing` com journald. Nível `info` para mudanças de estado (perfil,
controle, fonte de energia, override, emergência). `debug` para cada escrita no
sysfs. Não logue cada tick em `info`.

---

## 9. API D-Bus

Bus de sessão. Todos os valores de ventoinha usam `fan` = `"cpu"`, `"gpu"` ou
`"all"` (este último só nas chamadas de escrita).

```xml
<node>
  <interface name="io.github.lorenzopasquali.AlienFan1">
    <!-- Propriedades: todas emitem PropertiesChanged -->
    <property name="Version"             type="s"  access="read"/>
    <property name="Health"              type="s"  access="read"/> <!-- ok|degraded|emergency|no-permission|no-driver -->
    <property name="HealthMessage"       type="s"  access="read"/> <!-- texto pt-BR para a UI -->
    <property name="PowerSource"         type="s"  access="read"/> <!-- ac|battery -->
    <property name="Profile"             type="s"  access="read"/> <!-- perfil lido do sysfs -->
    <property name="AvailableProfiles"   type="as" access="read"/>
    <property name="Control"             type="s"  access="read"/> <!-- firmware|fixed|curve -->
    <property name="ActiveCurve"         type="s"  access="read"/> <!-- "" se não for curve -->
    <property name="OverrideActive"      type="b"  access="read"/>
    <property name="BoostRequiresCustom" type="b"  access="read"/>

    <!-- Leitura -->
    <method name="GetTelemetry">
      <arg name="fans"  type="aa{sv}" direction="out"/>
      <arg name="temps" type="a{sd}"  direction="out"/>
    </method>
    <method name="GetDefaults">
      <arg name="ac"      type="a{sv}" direction="out"/>
      <arg name="battery" type="a{sv}" direction="out"/>
    </method>
    <method name="ListCurves">
      <arg name="names" type="as" direction="out"/>
    </method>
    <method name="GetCurve">
      <arg name="name"   type="s"     direction="in"/>
      <arg name="fan"    type="s"     direction="in"/>
      <arg name="points" type="a(dy)" direction="out"/>
    </method>

    <!-- Escrita que cria override -->
    <method name="SetProfile">
      <arg name="profile" type="s" direction="in"/>
    </method>
    <method name="SetFixedBoost">          <!-- implica Control=fixed -->
      <arg name="fan"   type="s" direction="in"/>
      <arg name="boost" type="y" direction="in"/>
    </method>
    <method name="SetControl">             <!-- firmware|fixed|curve -->
      <arg name="control" type="s" direction="in"/>
      <arg name="curve"   type="s" direction="in"/> <!-- nome; ignorado se não for curve -->
    </method>
    <method name="RestoreDefault"/>

    <!-- Persistência -->
    <method name="SaveAsDefault">          <!-- ac|battery|both -->
      <arg name="target" type="s" direction="in"/>
    </method>
    <method name="SetDefault">             <!-- edita o padrão sem aplicar -->
      <arg name="target" type="s"    direction="in"/>
      <arg name="preset" type="a{sv}" direction="in"/> <!-- profile:s, control:s, fixed_cpu:y, fixed_gpu:y, curve:s -->
    </method>
    <method name="SaveCurve">              <!-- cria ou substitui [curves.<name>] -->
      <arg name="name" type="s"     direction="in"/>
      <arg name="cpu"  type="a(dy)" direction="in"/>
      <arg name="gpu"  type="a(dy)" direction="in"/>
    </method>
    <method name="DeleteCurve">            <!-- erro se estiver em uso em algum padrão -->
      <arg name="name" type="s" direction="in"/>
    </method>
    <method name="ReloadConfig"/>

    <!-- Emitido a cada tick -->
    <signal name="Telemetry">
      <arg name="fans"  type="aa{sv}"/>
      <arg name="temps" type="a{sd}"/>
    </signal>
  </interface>
</node>
```

Cada item de `fans` tem estas chaves:

| Chave | Tipo | Conteúdo |
|---|---|---|
| `id` | `s` | `cpu` ou `gpu` |
| `label` | `s` | label do sysfs |
| `rpm` | `u` | RPM atual |
| `rpm_max` | `u` | RPM máximo |
| `boost` | `y` | boost escrito |
| `target_boost` | `y` | boost que a curva quer (antes da rampa) |
| `temp_c` | `d` | temperatura do sensor que controla a ventoinha |
| `sensor` | `s` | nome do sensor |

`temps` é um mapa `"<hwmon>:<label>" → °C` com todos os sensores conhecidos (seção 2.2).

Erros (nomes D-Bus):

- `io.github.lorenzopasquali.AlienFan1.Error.InvalidArgument`
- `...Error.PermissionDenied`
- `...Error.HardwareUnavailable`
- `...Error.ConfigWriteFailed`
- `...Error.CurveInUse`

A mensagem de cada erro vai em pt-BR.

Para manter a API estável, só adicione. Uma quebra de API vira a interface
`AlienFan2`.

---

## 10. CLI `alienfan`

Se o daemon estiver no bus, a CLI fala com ele. Se não estiver, age direto no
sysfs e avisa na stderr: `aviso: daemon parado; mudança não será mantida pela
curva nem na troca de energia`. As exceções são `apply` e `doctor`, que sempre
agem sozinhas.

```
alienfan status [--json] [--watch]
    Perfil, controle, override, fonte de energia, RPM, temperaturas e saúde.
alienfan profile list
alienfan profile set <perfil>
alienfan boost <cpu|gpu|all> <0-255|0-100%>
alienfan control <firmware|fixed|curve> [--curve <nome>]
alienfan curve list
alienfan curve show <nome>
alienfan curve set <nome> --cpu "45:0,60:20%,70:45%,90:100%" [--gpu ...]
                          # temp:boost, onde o boost é bruto ou com %
alienfan default show
alienfan default save [--ac|--battery|--both]   # padrão: fonte atual
alienfan default reset                          # = RestoreDefault
alienfan apply [--boot] [--wait <segundos>]
    Lê a config e aplica o padrão da fonte atual, direto no sysfs.
    Com --boot: espera até --wait (padrão 10 s) pelos nós do sysfs. Não escreve
    nada que dependa da curva (usa boost 0). Sempre sai com 0.
alienfan doctor
```

O `doctor` confere e sugere a correção de cada item:

- driver carregado;
- nós do sysfs presentes;
- dono e permissão dos nós;
- usuário no grupo `alienfan`, e se a sessão atual já tem o grupo (pode faltar
  refazer o login);
- drop-in do TLP presente;
- `awccd` ativo (conflito);
- daemon ativo;
- extensão habilitada;
- config válida.

Saída legível em pt-BR, com `--json` para scripts.

Códigos de saída:

| Código | Significado |
|---|---|
| 0 | ok |
| 1 | erro genérico |
| 2 | argumento inválido |
| 3 | sem permissão |
| 4 | hardware não encontrado |
| 5 | erro do daemon |

---

## 11. Permissões, instalação e remoção

### 11.1 Regra udev (`packaging/udev/71-alienfan.rules`)

```udev
# Dá ao grupo alienfan escrita só nos nós usados pelo alienfan.
ACTION=="add|change", SUBSYSTEM=="platform-profile", ATTR{name}=="alienware-wmi", \
  RUN+="/bin/sh -c 'chgrp alienfan /sys%p/profile && chmod 0664 /sys%p/profile'"
ACTION=="add|change", SUBSYSTEM=="hwmon", ATTR{name}=="alienware_wmi", \
  RUN+="/bin/sh -c 'chgrp alienfan /sys%p/fan1_boost /sys%p/fan2_boost && chmod 0664 /sys%p/fan1_boost /sys%p/fan2_boost'"
```

Os nomes de subsystem e os atributos foram conferidos com `udevadm info`. A
permissão se perde quando o módulo é recarregado, e a regra a aplica de novo no
próximo `add`.

Para validar:

```bash
sudo udevadm trigger -c add -s platform-profile
sudo udevadm trigger -c add -s hwmon
```

Depois confira com `ls -l`.

### 11.2 Units systemd

`/etc/systemd/system/alienfan-apply.service`:

```ini
[Unit]
Description=alienfan: aplica o perfil térmico padrão no boot
After=systemd-udev-settle.service tlp.service
Wants=systemd-udev-settle.service

[Service]
Type=oneshot
ExecStart=/usr/local/bin/alienfan apply --boot --wait 10
RemainAfterExit=yes

[Install]
WantedBy=multi-user.target
```

O `systemd-udev-settle` está obsoleto. Se o agente preferir, troque por
`--wait` com polling no próprio binário, que já existe, e tire a dependência.

`/usr/lib/systemd/user/alienfand.service`:

```ini
[Unit]
Description=alienfan daemon (ventoinhas Alienware)

[Service]
Type=dbus
BusName=io.github.lorenzopasquali.AlienFan
ExecStart=/usr/local/bin/alienfand
Restart=on-failure
RestartSec=2

[Install]
WantedBy=default.target
```

### 11.3 Drop-in do TLP (`/etc/tlp.d/99-alienfan.conf`)

```sh
# alienfan controla o perfil térmico. Valor vazio = TLP não mexe.
PLATFORM_PROFILE_ON_AC=""
PLATFORM_PROFILE_ON_BAT=""
```

Depois de instalar, rode `sudo tlp start`. Confira que o `tlp-stat -p` não
mostra o TLP escrevendo `platform_profile`. O `/etc/tlp.conf` tem prioridade
sobre o `tlp.d`, e hoje essas linhas estão comentadas lá. O `doctor` deve avisar
se alguém descomentar.

### 11.4 `install.sh`

Roda como usuário e chama `sudo` só onde precisa. Imprima antes cada comando que
usa sudo. Etapas:

1. Checar os pré-requisitos (`cargo`, `npm`, pacotes de build do Tauri) e
   compilar em release. Nada aqui usa root.
2. `sudo groupadd -f alienfan` e `sudo usermod -aG alienfan "$USER"`.
3. Instalar os binários em `/usr/local/bin` (0755).
4. Instalar a regra udev e rodar `udevadm control --reload` e os `trigger`.
5. Criar `/etc/alienfan` (2775 `root:alienfan`) e copiar a config padrão (0664),
   **só se ela ainda não existir**.
6. Instalar o drop-in do TLP e rodar `tlp start`.
7. Instalar e habilitar o `alienfan-apply.service`.
8. Instalar a unit de usuário, o arquivo de ativação D-Bus, o `.desktop` e os
   ícones. Depois `systemctl --user daemon-reload` e `enable --now alienfand`.
9. Copiar a extensão para `~/.local/share/gnome-shell/extensions/` e compilar os
   schemas, se houver.
10. Rodar `alienfan doctor` no fim.
11. Avisar que é preciso **sair e entrar de novo** (para o grupo valer) e
    reiniciar o Shell (`Alt+F2` → `r` no X11), e depois habilitar a extensão.

Deve ser idempotente: rodar duas vezes não quebra nada.

### 11.5 `uninstall.sh`

Desfaz tudo na ordem inversa:

1. Desabilitar e remover a extensão e as units.
2. Remover a regra udev e rodar `trigger` para voltar a `root 0644`.
3. Remover o drop-in do TLP e rodar `tlp start`.
4. Remover binários, `.desktop` e ícones.
5. Perguntar antes de apagar `/etc/alienfan` e o grupo.

---

## 12. Extensão GNOME (Configurações Rápidas)

### 12.1 Metadados

```json
{
  "uuid": "alienfan@lorenzopasquali.github.io",
  "name": "Alienfan",
  "description": "Perfil térmico e ventoinhas Alienware",
  "shell-version": ["46"],
  "url": "file:///git/alienfan"
}
```

Mantenha `shell-version` só com as versões testadas (seção 2.4).

### 12.2 Estrutura (ESM, GNOME 46)

- `extension.js` exporta `default class extends Extension` com `enable()` e
  `disable()`.
- Imports permitidos:
  - `resource:///org/gnome/shell/extensions/extension.js`
  - `resource:///org/gnome/shell/ui/main.js`
  - `resource:///org/gnome/shell/ui/quickSettings.js` (`QuickMenuToggle`, `SystemIndicator`)
  - `resource:///org/gnome/shell/ui/popupMenu.js`
  - `resource:///org/gnome/shell/ui/slider.js`
  - `gi://Gio`, `gi://GLib`, `gi://St`, `gi://GObject`, `gi://Clutter`
- Registro: `Main.panel.statusArea.quickSettings.addExternalIndicator(indicator)`.
- D-Bus: use `Gio.DBusProxy.makeProxyWrapper(xml)` com o XML da seção 9, em
  `dbus.js`. Crie o proxy de forma assíncrona. Observe o dono do nome
  (`g-name-owner`).
- O `disable()` desfaz tudo: desconecta sinais, remove fontes GLib, destrói o
  indicador e o proxy. Siga as regras de review do extensions.gnome.org, mesmo
  sem publicar.

### 12.3 Comportamento

**O toggle ("Ventoinhas")**

- Ícone: `icons/alienfan-symbolic.svg` (hélice de 5 pás, symbolic).
- Título: `Ventoinhas`. Subtítulo: nome amigável do perfil efetivo, mais
  `· Curva <nome>` ou `· Boost N%` quando for o caso.
- `checked` = `OverrideActive`. Clicar no toggle com override ativo chama
  `RestoreDefault`. Sem override, o clique abre o menu.

**Menu (`QuickMenuToggle.menu`)**

1. Cabeçalho `setHeader(icon, "Ventoinhas", "<Na tomada|Na bateria> · <saúde>")`.
2. Seção **Perfil**: um item por `AvailableProfiles`, com ornamento de check no
   atual. Nomes amigáveis:

   | Perfil | Nome na UI |
   |---|---|
   | `cool` | Frio |
   | `quiet` | Silencioso |
   | `balanced` | Equilibrado |
   | `balanced-performance` | Equilibrado+ |
   | `performance` | Desempenho (G-Mode) |
   | `custom` | Personalizado |

   Se `BoostRequiresCustom` for `true` e o controle não for `firmware`, os itens
   ficam insensíveis, com a nota "Boost ativo usa Personalizado".
3. Seção **Controle**: itens `Automático (firmware)`, `Manual` e
   `Curva: <nome> ▸` (submenu com `ListCurves`).
4. Seção **Boost** (visível só quando o controle é `fixed`): dois sliders
   (`Slider` de `ui/slider.js`) rotulados `CPU` e `GPU`, cada um com
   `NN% · RPM`. Espere 150 ms parado antes de chamar `SetFixedBoost`.
5. Linha de telemetria: `CPU 62 °C · 2400 rpm   GPU 41 °C · 0 rpm`. Assine o
   `Telemetry` **só enquanto o menu estiver aberto**.
6. Ações:
   - `Salvar como padrão (<fonte atual>)` chama `SaveAsDefault`.
   - `Voltar ao padrão` chama `RestoreDefault`.
   - `Abrir painel…` usa `Gio.DesktopAppInfo.new('io.github.lorenzopasquali.AlienFan.desktop')?.launch([], null)`,
     com fallback em `Gio.Subprocess` rodando `alienfan-panel`. Feche o menu.

**Daemon ausente:** subtítulo "Serviço parado". Todos os itens ficam
insensíveis, e aparece o item "Iniciar serviço", que roda
`systemctl --user start alienfand` via `Gio.Subprocess`.

**Health ≠ ok:** mostre o `HealthMessage` numa linha de destaque. Em `emergency`,
use uma classe CSS vermelha.

**Indicador no painel superior:** o ícone aparece só quando `OverrideActive` ou
`Health` ≠ ok. Isso fica configurável depois. Sem `prefs.js` na v1.

### 12.4 `stylesheet.css`

Use classes com prefixo `alienfan-`. Não sobrescreva estilos globais do Shell.
Respeite o tema claro/escuro do Zorin usando cores do tema (`-st-accent-color`
quando existir, com fallback). Não use valores fixos que quebrem o tema claro.

---

## 13. Painel `alienfan-panel` (Tauri 2)

### 13.1 Técnica

- Tauri 2. Frontend com Vite e **TypeScript sem framework**. Adote Svelte 5 só
  se o editor de curva ficar complexo demais, e justifique no README.
- Dependências de build no Ubuntu 24.04:

  ```
  libwebkit2gtk-4.1-dev build-essential curl wget file libxdo-dev libssl-dev
  libayatana-appindicator3-dev librsvg2-dev
  ```

- Lado Rust (`src-tauri`):
  - Usa o proxy do `alienfan-proto`.
  - Expõe comandos Tauri que espelham os métodos D-Bus (`set_profile`,
    `set_fixed_boost`, `set_control`, `restore_default`, `save_as_default`,
    `set_default`, `get_defaults`, `list_curves`, `get_curve`, `save_curve`,
    `delete_curve`, `get_telemetry`).
  - Uma task repassa o sinal `Telemetry` como evento `telemetry`, e o
    `PropertiesChanged` como evento `state`.
  - Se o daemon cair, emite `daemon` com `{connected: false}` e reconecta
    sozinho a cada 2 s.
- `tauri-plugin-single-instance`: abrir de novo só foca a janela existente.
- Janela: 1040×700, mínimo 760×560. Decoração nativa na v1.
- Idioma: pt-BR.
- A UI nunca fala com o sysfs. Tudo passa pelo daemon.

### 13.2 Telas

Uma janela com uma barra lateral ou abas: **Ventoinhas**, **Curvas**,
**Padrões**.

**Ventoinhas (tela principal)**

- Dois cartões grandes lado a lado (empilhados abaixo de 900 px): **CPU** e **GPU**.
- Cada cartão tem:
  - A ventoinha animada (13.3).
  - Um anel em volta da ventoinha com `rpm / rpm_max`.
  - RPM grande (`font-variant-numeric: tabular-nums`) e a temperatura com a cor
    da faixa (13.4).
  - Um chip com o controle atual (`Automático`, `Manual NN%`, `Curva <nome>`).
  - Slider de boost 0–100%, botões `−10%` e `+10%`, e o botão
    `Automático (firmware)`. Mexer no slider muda para `fixed`, esperando
    150 ms parado antes de chamar o daemon.
  - Com a curva ativa, o slider mostra o `target_boost`, fica travado, e um
    botão "Assumir manualmente" muda para `fixed`.
  - Se a GPU estiver com 0 RPM e boost 0, uma nota: "Ventoinha da GPU parada:
    normal com a GPU ociosa".
- Faixa de **perfil** acima dos cartões: um controle segmentado com os 6 perfis
  (ícone, nome amigável e dica de uma linha). O `performance` leva o selo
  **G-Mode**.
  - Dica embutida: "Para deixar mais silencioso, use Silencioso ou Frio. O boost
    só acelera."
- Rodapé:
  - fonte de energia (ícone de tomada ou bateria);
  - saúde;
  - daemon conectado ou não;
  - "Override ativo", com os botões `Voltar ao padrão` e
    `Salvar como padrão (<fonte>)`.
- Banner no topo quando `Health` for `no-permission` ou `no-driver`, com o texto
  do `HealthMessage` e o comando `alienfan doctor`.

**Curvas**

- Lista das curvas, com botões de criar, duplicar e apagar. Apagar fica
  desabilitado se a curva estiver em uso.
- O editor é um gráfico SVG:
  - eixo X de 20 a 100 °C, eixo Y de 0 a 100% de boost;
  - uma linha por ventoinha (CPU e GPU), com a opção "mesma curva para as duas";
  - pontos arrastáveis, que respeitam as regras da seção 6.2 durante o arraste
    (a temperatura fica presa entre os vizinhos e o boost não fica abaixo do
    ponto anterior);
  - duplo clique adiciona um ponto, e `Delete` ou botão direito remove (mínimo 2);
  - marcador ao vivo com a temperatura atual e o boost real;
  - faixa hachurada acima de `emergency_temp_c`.
- Campos numéricos para histerese e rampas, com texto de ajuda.
- Presets de partida: Silencioso, Equilibrado e Agressivo (valores da seção 7.2).
- `Salvar` chama `SaveCurve`. `Testar agora` chama `SetControl("curve", nome)`
  e cria um override.

**Padrões**

- Duas colunas: **Na tomada** e **Na bateria**.
- Cada uma tem um editor de preset: perfil, controle, boost fixo (CPU e GPU) e
  curva.
- `Salvar padrões` chama `SetDefault` duas vezes. `Copiar estado atual para…`
  chama `SaveAsDefault`.
- Opção `override_until` ("Ao trocar tomada/bateria, voltar ao padrão"). Isso
  pede um método `SetDaemonOption` na v1.1, ou pode ficar só no arquivo na v1.
  Decida e documente.

### 13.3 Ventoinha animada

- Um SVG por ventoinha: aro externo, 7 pás curvas (path com bezier), cubo
  central e um brilho radial atrás.
- A rotação é feita **em JS com `requestAnimationFrame`**, não com
  `animation-duration`, porque mudar a duração da animação CSS faz a hélice
  pular.
  - `ω_alvo` (voltas por segundo) = `clamp(rpm / 450, 0, 7)`. A velocidade visual
    é reduzida de propósito: 6000 rpm reais seriam um borrão.
  - `ω` segue o `ω_alvo` com constante de tempo de 600 ms, para acelerar e frear
    com suavidade.
  - `ângulo += 360 * ω * dt`, aplicado com `transform: rotate()` no grupo das pás.
- Borrão de movimento: acima de 3 voltas/s, mostre uma cópia das pás com
  `opacity` proporcional e `filter: blur(1.5px)`, girando um pouco atrás.
- O brilho (`drop-shadow` e opacidade do fundo radial) fica proporcional a
  `rpm / rpm_max`.
- Com `prefers-reduced-motion: reduce`, não há rotação. As pás ficam paradas e o
  anel e os números continuam atualizando.
- Com a janela escondida (`document.hidden`), pause o rAF.

### 13.4 Direção visual ("CSS elaborado")

- Tema escuro por padrão ("cockpit"). O tema claro segue
  `prefers-color-scheme`.
- Defina tudo como variáveis CSS em `:root`, com as versões clara e escura.
- Fundo: `radial-gradient` de `#0b1020` a `#05070d`, com um ruído sutil (SVG
  `feTurbulence` inline, opacidade 0.04).
- Cartões em vidro:
  - `background: rgba(255,255,255,0.04)`;
  - `backdrop-filter: blur(16px)` com o prefixo `-webkit-` (é WebKitGTK; teste,
    e sem suporte use um fundo sólido);
  - borda `1px rgba(255,255,255,0.08)`, raio 20 px.
- Cor de destaque pela temperatura do cartão, com transição de 400 ms:

  | Temperatura | Cor |
  |---|---|
  | < 60 °C | ciano `#22d3ee` |
  | 60–80 °C | âmbar `#f59e0b` |
  | > 80 °C | vermelho `#ef4444` |

  O anel de RPM, o brilho e o polegar do slider usam essa cor.
- O anel de RPM usa `conic-gradient` com máscara radial, e o ponteiro tem uma
  transição suave.
- Tipografia:
  - Números em `"JetBrains Mono", ui-monospace` (fonte local, sem CDN).
  - Texto em `"Inter", system-ui`. Empacote as fontes no app se usar. Sem
    Google Fonts em tempo de execução, porque o app funciona offline.
- Microinterações:
  - Botões com `transform: translateY(-1px)` e brilho no hover.
  - Controle segmentado com um indicador deslizante animado.
- Emergência: borda pulsante vermelha nos cartões, desligada com reduced-motion.
- Contraste mínimo AA em texto nos dois temas.

### 13.5 Acessibilidade

- Todos os controles funcionam pelo teclado (Tab, setas nos sliders e no
  controle segmentado, Enter e Espaço).
- `aria-label` em pt-BR.
- Os pontos do editor de curva podem ser focados e movidos com as setas: ±1 °C
  na horizontal, ±1% na vertical, e Shift multiplica por 5.

---

## 14. Segurança

- Não existe processo root persistente. O único root é o
  `alienfan-apply.service`, que é oneshot, só lê a config e escreve em dois tipos
  de nó do sysfs.
- O grupo `alienfan` dá escrita **só** em `profile`, `fan1_boost` e `fan2_boost`,
  e na config. Estar no grupo equivale a controlar ventoinha e perfil. Não dá
  mais nada.
- **Toda validação acontece no daemon.** Extensão, painel e CLI não são
  confiáveis.
- O D-Bus de sessão é acessível a qualquer processo do usuário. Isso é aceito,
  porque o limite de confiança é o próprio usuário.
- O `apply --boot` roda como root e lê um arquivo que o grupo pode escrever. Ele
  só aceita valores validados (perfil do `choices` e boost 0–255) e nunca executa
  nada vindo da config. Nenhum caminho sai da config.
- O daemon nunca escreve fora dos nós descobertos e da pasta `/etc/alienfan`.

---

## 15. Remoção do `awcc` (tr1xem)

Ele foi instalado a partir de `/git/AWCC` (CMake). O manifesto
`/git/AWCC/build/install_manifest.txt` lista:

```
/usr/bin/awcc
/usr/share/applications/awcc.desktop
/usr/share/icons/awcc.png
/etc/udev/rules.d/70-awcc.rules      # regras USB 187c:0550/0551 (LEDs)
/etc/systemd/system/awccd.service
/etc/awcc/database.json
```

Passos (sudo; confirme com o usuário antes):

```bash
sudo systemctl disable --now awccd.service
sudo rm /usr/bin/awcc /usr/share/applications/awcc.desktop /usr/share/icons/awcc.png \
        /etc/udev/rules.d/70-awcc.rules /etc/systemd/system/awccd.service
sudo rm -r /etc/awcc
sudo systemctl daemon-reload && sudo udevadm control --reload
```

Não apague `/git/AWCC`: é o código-fonte clonado e serve de referência. O
`awccd` fazia KeyBinder em `/dev/input/event3`. Antes de remover, descubra se
alguma tecla do teclado (tecla AWCC ou `Fn+F?`) dependia dele. Use
`sudo libinput debug-events` ou `evtest` com o `awccd` parado. Se sim, registre
como questão em aberto (seção 18).

---

## 16. Testes e critérios de aceite

### 16.1 Automatizados (`cargo test`, sem hardware)

- **Fixture** `tests/fixtures/fake-sysfs/`: reproduz a árvore real da seção 2.2.
  - `class/platform-profile/platform-profile-0/{name,choices,profile}`
  - `class/hwmon/hwmon3/{name=alienware_wmi, fan{1,2}_{label,input,min,max,boost}, temp{1,2}_{label,input}}`
  - `class/hwmon/hwmon9/name=dell_smm`, para garantir que ele é ignorado
  - `class/power_supply/AC/{type,online}`

  Use números de hwmon **diferentes** dos reais, para provar que a descoberta usa
  o `name`.
- `core`:
  - descoberta, incluindo quando o driver não existe;
  - leitura e escrita;
  - escrita idempotente (sem escrita quando o valor não muda);
  - ordem perfil → boost;
  - `boost_requires_custom`;
  - conversão entre porcentagem e boost;
  - validação de curva (cada regra da seção 6.2 com um caso que falha);
  - interpolação (abaixo, entre e acima dos pontos);
  - round-trip da config, e config inválida (linha e coluna no erro).
- Motor da curva:
  - simulação com uma série de temperaturas pré-definida, conferindo a rampa de
    subida e de descida, a histerese, o limite de escrita (±2), a emergência
    (entrada e saída) e a falha de sensor (3 leituras levam ao boost 0);
  - usa relógio injetável, sem `sleep` real.
- Daemon:
  - teste de integração com `dbus-run-session` e `ALIENFAN_SYSFS_ROOT`
    apontando para uma cópia temporária da fixture;
  - chamar cada método e conferir o arquivo resultante e as propriedades;
  - simular a troca de energia escrevendo no `AC/online`, usando o fallback de
    polling.
- CLI: `--json` estável (snapshot), e os códigos de saída.
- Gates: `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`,
  `cargo test`, e `npm run build` e `tsc --noEmit` no painel.

### 16.2 Aceite manual no hardware

1. Depois do `install.sh` e de refazer o login, `alienfan doctor` fica todo verde.
2. `alienfan profile set quiet` funciona sem sudo. O perfil muda e o RPM cai em
   até 30 s.
3. `alienfan boost all 60%` sobe o RPM de forma visível (respeitando o
   resultado da Fase 0).
4. Tirar da tomada aplica o padrão da bateria. Ligar de novo aplica o da tomada.
   O TLP não sobrescreve (conferir no `journalctl -u tlp`).
5. Reboot: antes do login o perfil já é o padrão (conferir pelo TTY ou pelo log
   do `alienfan-apply`).
6. Suspender e voltar: o estado é reaplicado em até 3 s.
7. Com a curva `equilibrado` e uma carga (`stress-ng --cpu 0 -t 120`), o boost
   sobe de forma suave e desce devagar depois da carga.
8. `systemctl --user stop alienfand`: o boost volta a 0. A extensão mostra
   "Serviço parado".
9. A extensão não gera nenhum erro em
   `journalctl --user -b -o cat /usr/bin/gnome-shell | grep -i alienfan` depois
   de habilitar, desabilitar e reiniciar o Shell.
10. O painel abre pelo menu. As ventoinhas giram proporcionais ao RPM. Com
    reduced-motion elas ficam paradas.
11. `uninstall.sh` deixa o sistema como antes. O TLP volta a controlar o perfil.

---

## 17. Plano de implementação (marcos)

Cada marco termina com os gates verdes, um commit, e as partes que precisam de
sudo entregues ao usuário como comandos prontos. O agente não tem sudo.

| Marco | Entrega | Pronto quando |
|---|---|---|
| **M0** | Fase 0 (seção 4) com o usuário; `docs/HARDWARE.md` preenchido; `boost_requires_custom` decidido | Tabela T1–T7 preenchida |
| **M1** | Rust via rustup (sem root); workspace; `alienfan-core` + fixture + testes | `cargo test` verde |
| **M2** | CLI em modo direto; `packaging/` (udev, grupo, TLP, `alienfan-apply`, config); `install.sh` parcial | Aceite 1, 2, 3 e 5 |
| **M3** | `alienfand` + D-Bus + UPower + logind + curva + inotify; CLI via D-Bus; unit de usuário | Aceite 4, 6, 7 e 8; testes de integração |
| **M4** | Extensão GNOME | Aceite 9 |
| **M5** | Painel Tauri (Ventoinhas → Padrões → Curvas) | Aceite 10 |
| **M6** | Remoção do awcc (com confirmação), `uninstall.sh`, README, polimento | Aceite 11; README com instalação e troubleshooting |

---

## 18. Questões em aberto

1. **O boost exige o perfil `custom`?** E trocar de perfil zera o boost? Sai da
   Fase 0.
2. **Qual a base de RPM do perfil `custom`?** Se for alta, a curva com `custom`
   pode ficar mais barulhenta que `balanced` + firmware.
3. **Semântica do override:** o padrão sugerido é `power-change`. O usuário pode
   preferir `manual`.
4. **Tecla dedicada:** o notebook tem tecla de modo térmico ou AWCC? Se tiver,
   vale mapear para "próximo perfil" (pelo daemon, via `evdev`, o que pede
   leitura do `/dev/input` e outra permissão) ou só um atalho do GNOME chamando
   `alienfan profile next`. O mais simples é a segunda opção.
5. **Upgrade para GNOME 47+ (Zorin 19):** testar a extensão e só então somar ao
   `shell-version`.
6. **LEDs e teclado RGB:** fora do escopo da v1. O LightFX do awcc não achou o
   dispositivo.
7. **`dell_smm pwm1_enable = 1`:** o significado lido não foi verificado. Não
   escreva, só documente.
8. **Vários usuários ou sessões:** fora do escopo. Um daemon por usuário
   brigaria. Documente como limitação.

---

## 19. Fora do escopo (v1)

- Controle abaixo da base do firmware (via `dell_smm` ou EC direto).
- Overclock, limites de potência (PL1/PL2) e undervolt.
- Iluminação RGB.
- Outros modelos. O código não deve impedir, mas só este foi testado.
- Wayland: a extensão funciona igual. Só o `Alt+F2 → r` não existe lá. Documente
  e não teste na v1.
- Publicar no extensions.gnome.org.

---

## 20. Referências

- Doc do driver: `Documentation/admin-guide/laptops/alienware-wmi.rst` (kernel mainline).
- ABI do platform profile: `Documentation/ABI/testing/sysfs-class-platform-profile`.
- TLP 1.6: `/usr/share/tlp/func.d/10-tlp-func-cpu` (tratamento de `PLATFORM_PROFILE_*`).
- GNOME Shell 46, Quick Settings: `js/ui/quickSettings.js` no tag `46.0` do gnome-shell.
- Guia de extensões: https://gjs.guide/extensions/
- Tauri 2: https://v2.tauri.app/
- AWCC tr1xem (referência de modos WMI e UX): `/git/AWCC`.
