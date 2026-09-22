//! `alienfan doctor`: checks the installation and suggests a fix for each
//! problem (SPEC 10).

use std::fs;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

use alienfan_core::sysfs::is_writable;
use alienfan_core::{ConfigFile, Error, FanId, Hardware};
use serde::Serialize;

use crate::Ctx;

const GROUP: &str = "alienfan";
const UDEV_RULE: &str = "/etc/udev/rules.d/71-alienfan.rules";
const TLP_CONF: &str = "/etc/tlp.conf";
const TLP_DIR: &str = "/etc/tlp.d";
const TLP_DROPIN: &str = "99-alienfan.conf";
const EXTENSION_UUID: &str = "alienfan@lorenzopasquali.github.io";
const RETRIGGER: &str = "sudo udevadm trigger -c add -s platform-profile --attr-match=name=alienware-wmi && sudo udevadm trigger -c add -s hwmon --attr-match=name=alienware_wmi";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
enum Level {
    Ok,
    Warn,
    Fail,
}

#[derive(Debug, Serialize)]
struct Check {
    id: &'static str,
    level: Level,
    message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    fix: Option<String>,
}

impl Check {
    fn ok(id: &'static str, message: impl Into<String>) -> Self {
        Self {
            id,
            level: Level::Ok,
            message: message.into(),
            fix: None,
        }
    }

    fn warn(id: &'static str, message: impl Into<String>, fix: impl Into<String>) -> Self {
        Self {
            id,
            level: Level::Warn,
            message: message.into(),
            fix: Some(fix.into()),
        }
    }

    fn fail(id: &'static str, message: impl Into<String>, fix: impl Into<String>) -> Self {
        Self {
            id,
            level: Level::Fail,
            ..Self::warn(id, message, fix)
        }
    }
}

#[derive(Serialize)]
struct Report {
    ok: bool,
    checks: Vec<Check>,
}

pub fn run(ctx: &Ctx, json: bool) -> ExitCode {
    let hw = Hardware::discover(&ctx.root);
    let group = find_group(GROUP);
    let checks = vec![
        check_driver(ctx),
        check_nodes(&hw),
        check_permissions(&hw, group.as_ref()),
        check_group(group.as_ref()),
        check_tlp(),
        check_apply_service(),
        check_awccd(),
        check_daemon(),
        check_extension(),
        check_config(&ctx.config_path, group.as_ref()),
    ];
    let report = Report {
        ok: checks.iter().all(|c| c.level != Level::Fail),
        checks,
    };
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&report).expect("plain data serializes")
        );
    } else {
        for c in &report.checks {
            let mark = match c.level {
                Level::Ok => "✓",
                Level::Warn => "!",
                Level::Fail => "✗",
            };
            println!("{mark} {:<14} {}", c.id, c.message);
            if let Some(fix) = &c.fix {
                println!("  {:<14} → {fix}", "");
            }
        }
    }
    if report.ok {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

fn check_driver(ctx: &Ctx) -> Check {
    if ctx.root.path().join("module/alienware_wmi").exists() {
        Check::ok("driver", "módulo alienware_wmi carregado")
    } else {
        Check::fail(
            "driver",
            "módulo alienware_wmi não carregado",
            "sudo modprobe alienware_wmi",
        )
    }
}

fn check_nodes(hw: &Result<Hardware, Error>) -> Check {
    match hw {
        Ok(hw) => Check::ok(
            "nós do sysfs",
            format!(
                "{} e {}",
                hw.profile_path().display(),
                hw.boost_path(FanId::Cpu)
                    .with_file_name("fanN_boost")
                    .display()
            ),
        ),
        Err(e) => Check::fail(
            "nós do sysfs",
            e.to_string(),
            "confira `lsmod | grep alienware` e o `dmesg`",
        ),
    }
}

fn check_permissions(hw: &Result<Hardware, Error>, group: Option<&Group>) -> Check {
    const ID: &str = "permissões";
    let Ok(hw) = hw else {
        return Check::fail(
            ID,
            "sem nós do sysfs para conferir",
            "veja \"nós do sysfs\"",
        );
    };
    if !Path::new(UDEV_RULE).exists() {
        return Check::fail(
            ID,
            format!("regra udev ausente ({UDEV_RULE})"),
            "rode packaging/install.sh",
        );
    }
    let nodes = [
        hw.profile_path(),
        hw.boost_path(FanId::Cpu),
        hw.boost_path(FanId::Gpu),
    ];
    let wrong: Vec<String> = nodes
        .iter()
        .filter(|path| !group_writable(path, group))
        .map(|path| match fs::metadata(path) {
            Ok(m) => format!(
                "{} (gid {}, {:o})",
                path.display(),
                m.gid(),
                m.mode() & 0o777
            ),
            Err(_) => path.display().to_string(),
        })
        .collect();
    if !wrong.is_empty() {
        return Check::fail(
            ID,
            format!("sem escrita do grupo {GROUP}: {}", wrong.join(", ")),
            RETRIGGER,
        );
    }
    if nodes.iter().all(|p| is_writable(p)) {
        Check::ok(ID, format!("perfil e boost graváveis pelo grupo {GROUP}"))
    } else {
        Check::warn(
            ID,
            "os nós estão certos, mas esta sessão ainda não pode escrever",
            "veja \"grupo\"",
        )
    }
}

fn check_group(group: Option<&Group>) -> Check {
    const ID: &str = "grupo";
    let Some(group) = group else {
        return Check::fail(
            ID,
            format!("grupo {GROUP} não existe"),
            format!("sudo groupadd -f {GROUP}"),
        );
    };
    let user = user_name();
    if !group.members.iter().any(|m| m == &user) {
        return Check::fail(
            ID,
            format!("{user} não está no grupo {GROUP}"),
            format!("sudo usermod -aG {GROUP} {user}"),
        );
    }
    if session_gids().contains(&group.gid) {
        Check::ok(ID, format!("{user} está no grupo {GROUP} nesta sessão"))
    } else {
        Check::warn(
            ID,
            format!("{user} está no grupo {GROUP}, mas esta sessão é anterior"),
            "saia e entre de novo na sessão",
        )
    }
}

fn check_tlp() -> Check {
    const ID: &str = "tlp";
    if !Path::new(TLP_CONF).exists() && !Path::new(TLP_DIR).exists() {
        return Check::ok(ID, "TLP não instalado");
    }
    let dropin = Path::new(TLP_DIR).join(TLP_DROPIN);
    let Ok(text) = fs::read_to_string(&dropin) else {
        return Check::fail(
            ID,
            format!("{} ausente: o TLP ainda troca o perfil", dropin.display()),
            format!("sudo install -m 0644 packaging/tlp/{TLP_DROPIN} {TLP_DIR}/ && sudo tlp start"),
        );
    };
    if !profile_lines(&text).is_empty() {
        return Check::warn(
            ID,
            format!(
                "{} define um perfil; deveria deixar vazio",
                dropin.display()
            ),
            format!("reinstale packaging/tlp/{TLP_DROPIN}"),
        );
    }
    // TLP reads tlp.d in name order, then tlp.conf; the last value wins.
    let mut later: Vec<PathBuf> = fs::read_dir(TLP_DIR)
        .into_iter()
        .flatten()
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| {
            p.extension().is_some_and(|e| e == "conf")
                && p.file_name()
                    .is_some_and(|n| n.to_string_lossy().as_ref() > TLP_DROPIN)
        })
        .collect();
    later.sort();
    later.push(TLP_CONF.into());
    let overrides: Vec<String> = later
        .iter()
        .flat_map(|path| {
            let text = fs::read_to_string(path).unwrap_or_default();
            profile_lines(&text)
                .into_iter()
                .map(move |n| format!("{}:{n}", path.display()))
        })
        .collect();
    if overrides.is_empty() {
        Check::ok(ID, "drop-in presente; o TLP não mexe no perfil")
    } else {
        Check::warn(
            ID,
            format!(
                "PLATFORM_PROFILE_* definido depois do alienfan: {}",
                overrides.join(", ")
            ),
            "comente essas linhas e rode `sudo tlp start`",
        )
    }
}

/// Line numbers that set `PLATFORM_PROFILE_ON_AC/BAT` to a non-empty value.
fn profile_lines(text: &str) -> Vec<usize> {
    text.lines()
        .enumerate()
        .filter_map(|(i, line)| {
            let (key, value) = line.trim().split_once('=')?;
            let key = key.trim();
            let value = value.trim().trim_matches(['"', '\'']);
            (matches!(key, "PLATFORM_PROFILE_ON_AC" | "PLATFORM_PROFILE_ON_BAT")
                && !value.is_empty())
            .then_some(i + 1)
        })
        .collect()
}

fn check_apply_service() -> Check {
    const ID: &str = "boot";
    match systemctl(&["is-enabled", "alienfan-apply.service"]).as_deref() {
        Some("enabled") => Check::ok(ID, "alienfan-apply.service habilitado"),
        state => Check::fail(
            ID,
            format!(
                "alienfan-apply.service: {}",
                state.unwrap_or("desconhecido")
            ),
            "sudo systemctl enable alienfan-apply.service",
        ),
    }
}

fn check_awccd() -> Check {
    const ID: &str = "awccd";
    if systemctl(&["is-active", "awccd.service"]).as_deref() == Some("active") {
        Check::warn(
            ID,
            "awccd.service ativo (conflito com o alienfan)",
            "sudo systemctl disable --now awccd.service (SPEC, seção 15)",
        )
    } else {
        Check::ok(ID, "awccd não está rodando")
    }
}

fn check_daemon() -> Check {
    const ID: &str = "daemon";
    match systemctl(&["--user", "is-active", "alienfand.service"]).as_deref() {
        Some("active") => Check::ok(ID, "alienfand ativo"),
        state => Check::warn(
            ID,
            format!("alienfand: {}", state.unwrap_or("desconhecido")),
            "systemctl --user enable --now alienfand.service",
        ),
    }
}

fn check_extension() -> Check {
    const ID: &str = "extensão";
    let enabled = Command::new("gsettings")
        .args(["get", "org.gnome.shell", "enabled-extensions"])
        .output()
        .is_ok_and(|o| String::from_utf8_lossy(&o.stdout).contains(EXTENSION_UUID));
    if enabled {
        Check::ok(ID, "extensão habilitada")
    } else {
        Check::warn(
            ID,
            "extensão não habilitada",
            format!("gnome-extensions enable {EXTENSION_UUID}"),
        )
    }
}

fn check_config(path: &Path, group: Option<&Group>) -> Check {
    const ID: &str = "config";
    if !path.exists() {
        return Check::warn(
            ID,
            format!("{} ausente: usando os padrões embutidos", path.display()),
            "rode packaging/install.sh",
        );
    }
    match ConfigFile::load(path) {
        Err(e) => Check::fail(
            ID,
            format!("{}: {e}", path.display()),
            format!("corrija o arquivo ou restaure {}.bak", path.display()),
        ),
        Ok(_) if !group_writable(path, group) => Check::warn(
            ID,
            format!(
                "{} válida, mas sem escrita do grupo {GROUP}",
                path.display()
            ),
            format!(
                "sudo chgrp {GROUP} {0} && sudo chmod 0664 {0}",
                path.display()
            ),
        ),
        Ok(_) if !is_writable(path) => Check::warn(
            ID,
            format!(
                "{} válida, mas esta sessão ainda não pode salvar",
                path.display()
            ),
            "veja \"grupo\"",
        ),
        Ok(_) => Check::ok(ID, format!("{} válida", path.display())),
    }
}

/// Owned by the group, with the group write bit.
fn group_writable(path: &Path, group: Option<&Group>) -> bool {
    fs::metadata(path)
        .is_ok_and(|m| group.is_some_and(|g| g.gid == m.gid()) && m.mode() & 0o020 != 0)
}

fn systemctl(args: &[&str]) -> Option<String> {
    let out = Command::new("systemctl").args(args).output().ok()?;
    let state = String::from_utf8_lossy(&out.stdout).trim().to_owned();
    (!state.is_empty()).then_some(state)
}

struct Group {
    gid: u32,
    members: Vec<String>,
}

/// Looks `name` up in `/etc/group`.
fn find_group(name: &str) -> Option<Group> {
    fs::read_to_string("/etc/group")
        .ok()?
        .lines()
        .find_map(|line| {
            let mut f = line.split(':');
            if f.next()? != name {
                return None;
            }
            let gid = f.nth(1)?.parse().ok()?;
            let members = f
                .next()
                .unwrap_or_default()
                .split(',')
                .filter(|m| !m.is_empty())
                .map(str::to_owned)
                .collect();
            Some(Group { gid, members })
        })
}

/// Supplementary groups of this process, which a login grants once.
fn session_gids() -> Vec<u32> {
    fs::read_to_string("/proc/self/status")
        .unwrap_or_default()
        .lines()
        .find_map(|l| l.strip_prefix("Groups:"))
        .map(|g| {
            g.split_whitespace()
                .filter_map(|n| n.parse().ok())
                .collect()
        })
        .unwrap_or_default()
}

fn user_name() -> String {
    std::env::var("USER")
        .or_else(|_| std::env::var("LOGNAME"))
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_profile_settings() {
        let text = "# PLATFORM_PROFILE_ON_AC=performance\n\
                    PLATFORM_PROFILE_ON_AC=\"\"\n\
                    PLATFORM_PROFILE_ON_BAT=balanced\n\
                    PLATFORM_PROFILE_ON_AC = 'quiet'\n\
                    CPU_SCALING_GOVERNOR_ON_AC=performance\n";
        assert_eq!(profile_lines(text), [3, 4]);
    }
}
