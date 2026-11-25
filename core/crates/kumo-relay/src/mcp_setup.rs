use std::path::{Path, PathBuf};

use serde::Serialize;
use serde_json::Value;

const SERVER: &str = "kumo";

#[derive(Serialize, Debug, PartialEq)]
pub struct AgentSetup {
    pub agent: &'static str,
    pub added: bool,
    pub runs: Option<String>,
    pub add: String,
    pub remove: String,
}

#[derive(Serialize, Debug)]
pub struct McpSetup {
    pub binary: String,
    pub watch: String,
    pub agents: Vec<AgentSetup>,
}

pub fn current() -> McpSetup {
    let home = std::env::home_dir().unwrap_or_default();
    let codex_home = std::env::var_os("CODEX_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join(".codex"));
    let binary = binary_path();
    let quoted = shell_quote(&binary);
    let claude = claude_server(&read(&home.join(".claude.json")));
    let codex = codex_server(&read(&codex_home.join("config.toml")));
    McpSetup {
        agents: vec![
            AgentSetup {
                agent: "Claude Code",
                added: claude.is_some(),
                runs: claude,
                add: format!("claude mcp add --scope user {SERVER} -- {quoted}"),
                remove: format!("claude mcp remove --scope user {SERVER}"),
            },
            AgentSetup {
                agent: "Codex",
                added: codex.is_some(),
                runs: codex,
                add: format!("codex mcp add {SERVER} -- {quoted}"),
                remove: format!("codex mcp remove {SERVER}"),
            },
        ],
        watch: format!("{quoted} watch"),
        binary,
    }
}

fn read(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap_or_default()
}

fn binary_path() -> String {
    let name = if cfg!(windows) {
        "kumo-mcp.exe"
    } else {
        "kumo-mcp"
    };
    std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|dir| dir.join(name)))
        .filter(|path| path.is_file())
        .map(|path| path.display().to_string())
        .unwrap_or_else(|| "kumo-mcp".into())
}

fn shell_quote(text: &str) -> String {
    let plain = text
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || "/._-:\\".contains(c));
    if plain {
        text.into()
    } else if cfg!(windows) {
        format!("\"{text}\"")
    } else {
        format!("'{}'", text.replace('\'', r"'\''"))
    }
}

fn command_line(command: Option<&str>, args: &[String]) -> Option<String> {
    let command = command?;
    Some(
        std::iter::once(command)
            .chain(args.iter().map(String::as_str))
            .map(shell_quote)
            .collect::<Vec<_>>()
            .join(" "),
    )
}

fn claude_server(config: &str) -> Option<String> {
    let config: Value = serde_json::from_str(config).ok()?;
    let server = config.get("mcpServers")?.get(SERVER)?;
    let args: Vec<String> = server
        .get("args")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();
    command_line(server.get("command").and_then(Value::as_str), &args)
        .or_else(|| server.get("url").and_then(Value::as_str).map(String::from))
}

fn codex_server(config: &str) -> Option<String> {
    let config: toml::Table = toml::from_str(config).ok()?;
    let server = config.get("mcp_servers")?.get(SERVER)?;
    if server.get("enabled").and_then(toml::Value::as_bool) == Some(false) {
        return None;
    }
    let args: Vec<String> = server
        .get("args")
        .and_then(toml::Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();
    command_line(server.get("command").and_then(toml::Value::as_str), &args).or_else(|| {
        server
            .get("url")
            .and_then(toml::Value::as_str)
            .map(String::from)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn claude_code_shows_the_command_it_runs() {
        let config = r#"{"mcpServers":{"kumo":{"type":"stdio","command":"/Applications/Better Kumo.app/Contents/MacOS/kumo-mcp","args":["--read-only"]}}}"#;
        assert_eq!(
            claude_server(config).as_deref(),
            Some("'/Applications/Better Kumo.app/Contents/MacOS/kumo-mcp' --read-only")
        );
    }

    #[test]
    fn claude_code_without_kumo_is_not_added() {
        assert_eq!(
            claude_server(r#"{"mcpServers":{"other":{"command":"x"}}}"#),
            None
        );
        assert_eq!(claude_server(""), None);
    }

    #[test]
    fn codex_shows_the_command_it_runs() {
        let config = "[mcp_servers.kumo]\ncommand = \"/usr/bin/kumo-mcp\"\nargs = []\n";
        assert_eq!(codex_server(config).as_deref(), Some("/usr/bin/kumo-mcp"));
    }

    #[test]
    fn a_disabled_codex_server_is_not_added() {
        let config = "[mcp_servers.kumo]\ncommand = \"kumo-mcp\"\nenabled = false\n";
        assert_eq!(codex_server(config), None);
    }

    #[test]
    fn paths_with_spaces_are_quoted_for_the_shell() {
        assert_eq!(shell_quote("/usr/bin/kumo-mcp"), "/usr/bin/kumo-mcp");
        if !cfg!(windows) {
            assert_eq!(shell_quote("/A B/it's"), r"'/A B/it'\''s'");
        }
    }
}
