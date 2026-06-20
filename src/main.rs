use std::env;
use std::path::Path;
use std::path::PathBuf;
use std::process::Command;

use anyhow::anyhow;
use anyhow::bail;
use anyhow::Context;
use anyhow::Result;
use clap::Parser;
use clap::Subcommand;
use clap::ValueEnum;

#[derive(Parser)]
#[command(version, about = "Open diffview in an external interactive terminal")]
struct Args {
    /// Terminal backend to use for this launch.
    #[arg(long, value_enum)]
    terminal: Option<BackendArg>,

    #[command(subcommand)]
    command: Option<CliCommand>,

    /// Directory to open in diffview.
    #[arg(default_value = ".")]
    path: String,
}

#[derive(Debug, Subcommand)]
enum CliCommand {
    /// Read or update diffview configuration.
    Config {
        #[command(subcommand)]
        command: ConfigCommand,
    },
}

#[derive(Debug, Subcommand)]
enum ConfigCommand {
    /// Set the default terminal backend.
    SetTerminal {
        /// Terminal backend to use by default.
        terminal: BackendArg,
    },
    /// Print the configured default terminal backend.
    GetTerminal,
    /// Detect the terminal from the environment and set it as the default if
    /// none is configured yet, then print how to change it.
    Init,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
enum BackendArg {
    Tmux,
    Wezterm,
    Kitty,
    Ghostty,
    Alacritty,
    Iterm2,
    Terminal,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Backend {
    Tmux,
    Wezterm,
    Kitty,
    Ghostty,
    Alacritty,
    ITerm2,
    TerminalApp,
}

impl BackendArg {
    fn as_config_value(self) -> &'static str {
        match self {
            Self::Tmux => "tmux",
            Self::Wezterm => "wezterm",
            Self::Kitty => "kitty",
            Self::Ghostty => "ghostty",
            Self::Alacritty => "alacritty",
            Self::Iterm2 => "iterm2",
            Self::Terminal => "terminal",
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
struct LaunchCommand {
    program: String,
    args: Vec<String>,
}

fn main() -> Result<()> {
    let args = Args::parse();

    if let Some(CliCommand::Config { command }) = args.command {
        return handle_config_command(command);
    }

    let cwd = std::fs::canonicalize(&args.path)
        .with_context(|| format!("failed to resolve {}", args.path))?;
    let configured = read_default_terminal().transpose()?;
    let backend = select_backend_from_config(args.terminal, configured.as_deref())?;
    let command = build_launch_command(backend, &cwd);
    run_launch_command(command)
}

fn handle_config_command(command: ConfigCommand) -> Result<()> {
    match command {
        ConfigCommand::SetTerminal { terminal } => {
            write_default_terminal(terminal)?;
            println!("Default terminal set to {}.", terminal.as_config_value());
        }
        ConfigCommand::GetTerminal => match read_default_terminal().transpose()? {
            Some(terminal) => println!("{terminal}"),
            None => bail!(missing_terminal_config_message()),
        },
        ConfigCommand::Init => init_default_terminal()?,
    }
    Ok(())
}

/// Auto-configure the default terminal during install. Keeps any existing
/// choice so reinstalls are idempotent; otherwise detects from the current
/// environment. Prints a bilingual summary in every case.
fn init_default_terminal() -> Result<()> {
    if let Some(existing) = read_default_terminal().transpose()? {
        println!("{}", init_summary(existing.trim(), true));
        return Ok(());
    }
    match detect_backend_from_env() {
        Some(arg) => {
            write_default_terminal(arg)?;
            println!("{}", init_summary(arg.as_config_value(), false));
        }
        None => println!("{}", could_not_detect_message()),
    }
    Ok(())
}

/// Best-effort detection of the host terminal from well-known env vars.
fn detect_backend_from_env() -> Option<BackendArg> {
    detect_backend(|key| env::var(key).ok())
}

fn detect_backend(get: impl Fn(&str) -> Option<String>) -> Option<BackendArg> {
    // Prefer tmux when running inside it: the launcher splits the current window.
    if get("TMUX").is_some_and(|v| !v.is_empty()) {
        return Some(BackendArg::Tmux);
    }
    if let Some(tp) = get("TERM_PROGRAM") {
        let tp = tp.to_ascii_lowercase();
        if tp.contains("iterm") {
            return Some(BackendArg::Iterm2);
        }
        if tp.contains("apple_terminal") {
            return Some(BackendArg::Terminal);
        }
        if tp.contains("wezterm") {
            return Some(BackendArg::Wezterm);
        }
        if tp.contains("ghostty") {
            return Some(BackendArg::Ghostty);
        }
    }
    if get("KITTY_WINDOW_ID").is_some() {
        return Some(BackendArg::Kitty);
    }
    if get("WEZTERM_PANE").is_some() || get("WEZTERM_EXECUTABLE").is_some() {
        return Some(BackendArg::Wezterm);
    }
    if get("ALACRITTY_WINDOW_ID").is_some() || get("ALACRITTY_SOCKET").is_some() {
        return Some(BackendArg::Alacritty);
    }
    if get("GHOSTTY_RESOURCES_DIR").is_some() || get("GHOSTTY_BIN_DIR").is_some() {
        return Some(BackendArg::Ghostty);
    }
    None
}

const SUPPORTED_TERMINALS: &str = "tmux, wezterm, kitty, ghostty, alacritty, iterm2, terminal";

fn init_summary(value: &str, already: bool) -> String {
    let zh_lead = if already {
        format!("diffview：默认预览终端已设置为 “{value}”。")
    } else {
        format!("diffview：已根据当前环境将默认预览终端设置为 “{value}”。")
    };
    let en_lead = if already {
        format!("diffview: default preview terminal is already set to \"{value}\".")
    } else {
        format!(
            "diffview: default preview terminal set to \"{value}\" (detected from your environment)."
        )
    };
    format!(
        "{zh_lead}\n如需调整，请运行：diffview config set-terminal <terminal>\n支持的终端：{SUPPORTED_TERMINALS}\n\n\
         {en_lead}\nTo change it, run: diffview config set-terminal <terminal>\nSupported terminals: {SUPPORTED_TERMINALS}"
    )
}

fn could_not_detect_message() -> String {
    format!(
        "diffview：未能从当前环境识别终端类型。请运行：diffview config set-terminal <terminal> 进行设置。\n支持的终端：{SUPPORTED_TERMINALS}\n\n\
         diffview: could not detect a terminal from your environment. Run: diffview config set-terminal <terminal>\nSupported terminals: {SUPPORTED_TERMINALS}"
    )
}

fn select_backend_from_config(
    requested: Option<BackendArg>,
    configured: Option<&str>,
) -> Result<Backend> {
    if let Some(requested) = requested {
        return Ok(backend_from_arg(requested));
    }

    let Some(configured) = configured else {
        bail!(missing_terminal_config_message());
    };
    parse_backend(configured.trim())
}

fn backend_from_arg(arg: BackendArg) -> Backend {
    match arg {
        BackendArg::Tmux => Backend::Tmux,
        BackendArg::Wezterm => Backend::Wezterm,
        BackendArg::Kitty => Backend::Kitty,
        BackendArg::Ghostty => Backend::Ghostty,
        BackendArg::Alacritty => Backend::Alacritty,
        BackendArg::Iterm2 => Backend::ITerm2,
        BackendArg::Terminal => Backend::TerminalApp,
    }
}

fn parse_backend(value: &str) -> Result<Backend> {
    match value {
        "tmux" => Ok(Backend::Tmux),
        "wezterm" => Ok(Backend::Wezterm),
        "kitty" => Ok(Backend::Kitty),
        "ghostty" => Ok(Backend::Ghostty),
        "alacritty" => Ok(Backend::Alacritty),
        "iterm2" => Ok(Backend::ITerm2),
        "terminal" => Ok(Backend::TerminalApp),
        other => bail!("unsupported terminal backend `{other}`"),
    }
}

fn missing_terminal_config_message() -> String {
    "No default terminal is configured. Run `diffview config set-terminal <terminal>` first. Supported terminals: tmux, wezterm, kitty, ghostty, alacritty, iterm2, terminal.".to_string()
}

fn config_path() -> Result<PathBuf> {
    let base = env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))
        .ok_or_else(|| anyhow!("HOME is not set; cannot locate diffview config"))?;
    Ok(base.join("diffview").join("config"))
}

fn read_default_terminal() -> Option<Result<String>> {
    let path = match config_path() {
        Ok(path) => path,
        Err(err) => return Some(Err(err)),
    };
    match std::fs::read_to_string(path) {
        Ok(value) => {
            let value = value.trim().to_string();
            (!value.is_empty()).then_some(Ok(value))
        }
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => None,
        Err(err) => Some(Err(err.into())),
    }
}

fn write_default_terminal(terminal: BackendArg) -> Result<()> {
    let path = config_path()?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, format!("{}\n", terminal.as_config_value()))?;
    Ok(())
}

fn build_launch_command(backend: Backend, cwd: &Path) -> LaunchCommand {
    match backend {
        Backend::Tmux => LaunchCommand {
            program: "tmux".to_string(),
            args: vec![
                "split-window".to_string(),
                "-c".to_string(),
                cwd.display().to_string(),
                "exec diffview-tui .".to_string(),
            ],
        },
        Backend::Wezterm => LaunchCommand {
            program: "wezterm".to_string(),
            args: vec![
                "cli".to_string(),
                "spawn".to_string(),
                "--cwd".to_string(),
                cwd.display().to_string(),
                "diffview-tui".to_string(),
                ".".to_string(),
            ],
        },
        Backend::Kitty => LaunchCommand {
            program: "kitty".to_string(),
            args: vec![
                "--directory".to_string(),
                cwd.display().to_string(),
                "diffview-tui".to_string(),
                ".".to_string(),
            ],
        },
        Backend::Ghostty => LaunchCommand {
            program: "ghostty".to_string(),
            args: vec![
                "--working-directory".to_string(),
                cwd.display().to_string(),
                "-e".to_string(),
                "diffview-tui".to_string(),
                ".".to_string(),
            ],
        },
        Backend::Alacritty => LaunchCommand {
            program: "alacritty".to_string(),
            args: vec![
                "--working-directory".to_string(),
                cwd.display().to_string(),
                "-e".to_string(),
                "diffview-tui".to_string(),
                ".".to_string(),
            ],
        },
        Backend::ITerm2 => LaunchCommand {
            program: "osascript".to_string(),
            args: vec!["-e".to_string(), iterm2_script(cwd)],
        },
        Backend::TerminalApp => LaunchCommand {
            program: "osascript".to_string(),
            args: vec!["-e".to_string(), terminal_app_script(cwd)],
        },
    }
}

fn run_launch_command(command: LaunchCommand) -> Result<()> {
    let status = Command::new(&command.program)
        .args(&command.args)
        .status()
        .with_context(|| format!("failed to run {}", command.program))?;
    if status.success() {
        Ok(())
    } else {
        Err(anyhow!("{} exited with {status}", command.program))
    }
}

fn iterm2_script(cwd: &Path) -> String {
    let command = diffview_shell_command(cwd);
    format!(
        r#"tell application "iTerm2"
  activate
  if (count of windows) = 0 then
    create window with default profile
  else
    tell current window
      create tab with default profile
    end tell
  end if
  tell current session of current window
    write text {}
  end tell
end tell"#,
        applescript_string(&command)
    )
}

fn terminal_app_script(cwd: &Path) -> String {
    let command = diffview_shell_command(cwd);
    format!(
        r#"tell application "Terminal"
  activate
  do script {}
end tell"#,
        applescript_string(&command)
    )
}

fn diffview_shell_command(cwd: &Path) -> String {
    format!("cd {} && exec diffview-tui .", shell_quote(cwd))
}

fn shell_quote(path: &Path) -> String {
    let s = path.as_os_str().to_string_lossy();
    format!("'{}'", s.replace('\'', "'\\''"))
}

fn applescript_string(value: &str) -> String {
    format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\""))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shell_quote_wraps_single_quotes_safely() {
        assert_eq!(shell_quote(Path::new("/tmp/a'b")), "'/tmp/a'\\''b'");
    }

    #[test]
    fn applescript_string_escapes_backslashes_and_quotes() {
        assert_eq!(applescript_string("a\\b\"c"), "\"a\\\\b\\\"c\"");
    }

    #[test]
    fn tmux_command_uses_cwd_arg_and_shell_command_only_for_diffview_tui() {
        let cmd = build_launch_command(Backend::Tmux, Path::new("/tmp/work dir"));

        assert_eq!(
            cmd,
            LaunchCommand {
                program: "tmux".to_string(),
                args: vec![
                    "split-window".to_string(),
                    "-c".to_string(),
                    "/tmp/work dir".to_string(),
                    "exec diffview-tui .".to_string(),
                ],
            }
        );
    }

    #[test]
    fn iterm_command_cd_quotes_cwd_for_shell_inside_new_tab() {
        let cmd = build_launch_command(Backend::ITerm2, Path::new("/tmp/a'b"));

        assert_eq!(cmd.program, "osascript");
        assert_eq!(
            cmd.args,
            vec!["-e".to_string(), iterm2_script(Path::new("/tmp/a'b"))]
        );
        assert_eq!(
            diffview_shell_command(Path::new("/tmp/a'b")),
            "cd '/tmp/a'\\''b' && exec diffview-tui ."
        );
        assert!(
            cmd.args[1].contains("cd '/tmp/a'\\\\''b' && exec diffview-tui ."),
            "{}",
            cmd.args[1]
        );
    }

    #[test]
    fn terminal_app_command_uses_do_script_with_quoted_cwd() {
        let cmd = build_launch_command(Backend::TerminalApp, Path::new("/tmp/space dir"));

        assert_eq!(cmd.program, "osascript");
        assert!(cmd.args[1].contains("tell application \"Terminal\""));
        assert!(cmd.args[1].contains("cd '/tmp/space dir' && exec diffview-tui ."));
    }

    #[test]
    fn gui_terminal_commands_pass_cwd_as_arguments() {
        for backend in [
            Backend::Wezterm,
            Backend::Kitty,
            Backend::Ghostty,
            Backend::Alacritty,
        ] {
            let cmd = build_launch_command(backend, Path::new("/tmp/work dir"));
            assert!(cmd.args.contains(&"/tmp/work dir".to_string()));
            assert!(cmd.args.contains(&"diffview-tui".to_string()));
        }
    }

    #[test]
    fn missing_config_reports_setup_command() {
        let err = select_backend_from_config(None, None)
            .expect_err("default terminal should be required");

        assert!(err.to_string().contains("diffview config set-terminal"));
    }

    #[test]
    fn default_backend_comes_from_config() {
        assert_eq!(
            select_backend_from_config(None, Some("iterm2")).unwrap(),
            Backend::ITerm2
        );
    }

    #[test]
    fn explicit_terminal_overrides_config() {
        assert_eq!(
            select_backend_from_config(Some(BackendArg::Terminal), Some("iterm2"),).unwrap(),
            Backend::TerminalApp
        );
    }

    fn detect_with(pairs: &[(&str, &str)]) -> Option<BackendArg> {
        detect_backend(|key| {
            pairs
                .iter()
                .find(|(k, _)| *k == key)
                .map(|(_, v)| v.to_string())
        })
    }

    #[test]
    fn detect_prefers_tmux_when_inside_it() {
        assert_eq!(
            detect_with(&[
                ("TMUX", "/tmp/tmux-501/default,1,0"),
                ("TERM_PROGRAM", "iTerm.app")
            ]),
            Some(BackendArg::Tmux)
        );
    }

    #[test]
    fn detect_reads_term_program_variants() {
        assert_eq!(
            detect_with(&[("TERM_PROGRAM", "iTerm.app")]),
            Some(BackendArg::Iterm2)
        );
        assert_eq!(
            detect_with(&[("TERM_PROGRAM", "Apple_Terminal")]),
            Some(BackendArg::Terminal)
        );
        assert_eq!(
            detect_with(&[("TERM_PROGRAM", "WezTerm")]),
            Some(BackendArg::Wezterm)
        );
        assert_eq!(
            detect_with(&[("TERM_PROGRAM", "ghostty")]),
            Some(BackendArg::Ghostty)
        );
    }

    #[test]
    fn detect_falls_back_to_terminal_specific_vars() {
        assert_eq!(
            detect_with(&[("KITTY_WINDOW_ID", "1")]),
            Some(BackendArg::Kitty)
        );
        assert_eq!(
            detect_with(&[("ALACRITTY_WINDOW_ID", "1")]),
            Some(BackendArg::Alacritty)
        );
        assert_eq!(
            detect_with(&[("GHOSTTY_RESOURCES_DIR", "/x")]),
            Some(BackendArg::Ghostty)
        );
        assert_eq!(
            detect_with(&[("WEZTERM_PANE", "0")]),
            Some(BackendArg::Wezterm)
        );
    }

    #[test]
    fn detect_returns_none_for_unknown_environment() {
        assert_eq!(detect_with(&[("TERM", "xterm-256color")]), None);
        assert_eq!(detect_with(&[]), None);
    }

    #[test]
    fn init_summary_is_bilingual_and_shows_change_command() {
        let msg = init_summary("iterm2", false);
        assert!(msg.contains("默认预览终端"));
        assert!(msg.contains("default preview terminal"));
        assert!(msg.contains("iterm2"));
        assert!(msg.contains("diffview config set-terminal"));
    }
}
