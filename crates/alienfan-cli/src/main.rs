//! `alienfan`: command line for the alienfan thermal profile and fan control.
//!
//! Until the daemon exists (M3) every command acts on sysfs directly.

mod curves;
mod direct;
mod doctor;
mod status;

use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;

use alienfan_core::config::{self, ConfigFile};
use alienfan_core::{Boost, ControlKind, Error, FanId, Hardware, Profile, SysfsRoot};
use clap::{Args, Parser, Subcommand, ValueEnum};

/// Exit codes of SPEC 10.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Code {
    Generic = 1,
    InvalidArgument = 2,
    NoPermission = 3,
    NoHardware = 4,
    Daemon = 5,
}

/// A command that failed: message for stderr plus exit code.
#[derive(Debug)]
pub struct Failure {
    pub code: Code,
    pub message: String,
}

impl Failure {
    pub fn new(code: Code, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}

impl From<Error> for Failure {
    fn from(e: Error) -> Self {
        let code = match e {
            Error::Invalid(_) | Error::CurveInUse { .. } => Code::InvalidArgument,
            Error::NoPermission { .. } => Code::NoPermission,
            Error::NoDriver(_) => Code::NoHardware,
            Error::Config(_) | Error::Io { .. } => Code::Generic,
        };
        Self::new(code, e.to_string())
    }
}

pub type CliResult<T = ()> = Result<T, Failure>;

/// Where the hardware and the config live. Tests point both elsewhere.
pub struct Ctx {
    pub root: SysfsRoot,
    pub config_path: PathBuf,
}

impl Ctx {
    fn from_env() -> Self {
        Self {
            root: SysfsRoot::from_env(),
            config_path: config::config_path(),
        }
    }

    pub fn hardware(&self) -> CliResult<Hardware> {
        Ok(Hardware::discover(&self.root)?)
    }

    pub fn config(&self) -> CliResult<ConfigFile> {
        ConfigFile::load(&self.config_path).map_err(|e| match e {
            Error::Config(c) => Failure::new(
                Code::Generic,
                format!("config inválida em {}: {c}", self.config_path.display()),
            ),
            other => other.into(),
        })
    }
}

#[derive(Parser)]
#[command(
    name = "alienfan",
    version,
    about = "Perfil térmico e ventoinhas do Alienware",
    disable_help_subcommand = true
)]
struct Cli {
    /// Mostra cada escrita no sysfs
    #[arg(short, long, global = true)]
    verbose: bool,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Perfil, controle, fonte de energia, RPM, temperaturas e saúde
    Status {
        /// Saída em JSON (uma linha por leitura com --watch)
        #[arg(long)]
        json: bool,
        /// Atualiza a cada segundo até Ctrl+C
        #[arg(long)]
        watch: bool,
    },
    /// Perfis térmicos
    #[command(subcommand)]
    Profile(ProfileCommand),
    /// Acelera ventoinhas: 0–255 ou 0–100%
    Boost {
        fan: FanArg,
        /// Ex.: 153 ou 60%
        #[arg(allow_hyphen_values = true)]
        value: Boost,
    },
    /// Troca o controle das ventoinhas
    Control {
        kind: ControlKind,
        /// Curva usada com "curve"
        #[arg(long, value_name = "NOME")]
        curve: Option<String>,
    },
    /// Curvas temperatura → boost
    #[command(subcommand)]
    Curve(CurveCommand),
    /// Padrões da tomada e da bateria
    #[command(subcommand)]
    Default(DefaultCommand),
    /// Aplica o padrão da fonte de energia atual, direto no sysfs
    Apply {
        /// Modo do serviço de boot: espera o driver e sempre sai com 0
        #[arg(long)]
        boot: bool,
        /// Quanto esperar pelos nós do sysfs (padrão: 10 com --boot, 0 sem)
        #[arg(long, value_name = "SEGUNDOS")]
        wait: Option<u64>,
    },
    /// Confere a instalação e sugere correções
    Doctor {
        #[arg(long)]
        json: bool,
    },
}

#[derive(Subcommand)]
enum ProfileCommand {
    /// Lista os perfis disponíveis
    List,
    /// Troca o perfil térmico
    Set { profile: Profile },
}

#[derive(Subcommand)]
enum CurveCommand {
    /// Lista as curvas da config
    List,
    /// Mostra os pontos de uma curva
    Show { name: String },
    /// Cria ou substitui uma curva
    Set(curves::SetArgs),
}

#[derive(Subcommand)]
enum DefaultCommand {
    /// Mostra os padrões da tomada e da bateria
    Show,
    /// Salva o estado atual como padrão (padrão: fonte atual)
    Save(SaveTarget),
    /// Volta ao padrão da fonte atual
    Reset,
}

#[derive(Args)]
#[group(multiple = false)]
struct SaveTarget {
    #[arg(long)]
    ac: bool,
    #[arg(long)]
    battery: bool,
    #[arg(long)]
    both: bool,
}

#[derive(Clone, Copy, ValueEnum)]
pub enum FanArg {
    Cpu,
    Gpu,
    All,
}

impl FanArg {
    pub fn fans(self) -> &'static [FanId] {
        match self {
            Self::Cpu => &[FanId::Cpu],
            Self::Gpu => &[FanId::Gpu],
            Self::All => FanId::ALL,
        }
    }
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    if cli.verbose {
        tracing_subscriber::fmt()
            .with_max_level(tracing_subscriber::filter::LevelFilter::DEBUG)
            .with_writer(std::io::stderr)
            .init();
    }
    match run(cli.command, &Ctx::from_env()) {
        Ok(code) => code,
        Err(f) => {
            eprintln!("erro: {}", f.message);
            ExitCode::from(f.code as u8)
        }
    }
}

fn run(command: Command, ctx: &Ctx) -> CliResult<ExitCode> {
    match command {
        Command::Status { json, watch } => return status::run(ctx, json, watch),
        Command::Profile(ProfileCommand::List) => direct::profile_list(ctx)?,
        Command::Profile(ProfileCommand::Set { profile }) => direct::profile_set(ctx, profile)?,
        Command::Boost { fan, value } => direct::boost(ctx, fan.fans(), value)?,
        Command::Control { kind, curve } => direct::control(ctx, kind, curve.as_deref())?,
        Command::Curve(CurveCommand::List) => curves::list(ctx)?,
        Command::Curve(CurveCommand::Show { name }) => curves::show(ctx, &name)?,
        Command::Curve(CurveCommand::Set(args)) => curves::set(ctx, &args)?,
        Command::Default(DefaultCommand::Show) => direct::default_show(ctx)?,
        Command::Default(DefaultCommand::Save(t)) => {
            direct::default_save(ctx, t.ac || t.both, t.battery || t.both)?;
        }
        Command::Default(DefaultCommand::Reset) => direct::default_reset(ctx)?,
        Command::Apply { boot, wait } => {
            let wait = wait.unwrap_or(if boot { 10 } else { 0 });
            direct::apply(ctx, boot, Duration::from_secs(wait))?;
        }
        Command::Doctor { json } => return Ok(doctor::run(ctx, json)),
    }
    Ok(ExitCode::SUCCESS)
}
