//! Agent CLIs already installed on this computer, and how to run one.
//!
//! People who would use this scanner often already pay for a Claude or ChatGPT
//! subscription and have its CLI signed in. This module finds those CLIs and
//! runs them non-interactively, so the chat can use them instead of an API key.
//!
//! The approach follows Commonroom (`src/engine/process.mjs` there), which
//! drives the same CLIs on Windows with the person's own sign-in:
//!
//! * look on `PATH` **and** where installers really put them (npm's global
//!   folder, the Claude and Codex desktop apps' own copies), and when several
//!   copies exist use the newest;
//! * accept a full path from Settings when discovery misses one;
//! * never pass API-key variables to the child, so a stray
//!   `ANTHROPIC_API_KEY` cannot silently turn a subscription into a bill.
//!
//! Running a CLI is behind [`CliRunner`] so tests never spawn anything.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Which installed agent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LocalAgentKind {
    /// Anthropic's Claude Code (`claude`).
    ClaudeCode,
    /// OpenAI's Codex CLI (`codex`).
    Codex,
    /// Google's Gemini CLI (`gemini`).
    Gemini,
}

impl LocalAgentKind {
    /// Every kind, in the order Settings lists them.
    pub const ALL: [LocalAgentKind; 3] =
        [LocalAgentKind::ClaudeCode, LocalAgentKind::Codex, LocalAgentKind::Gemini];

    /// The command's name, without an extension.
    pub fn command(self) -> &'static str {
        match self {
            LocalAgentKind::ClaudeCode => "claude",
            LocalAgentKind::Codex => "codex",
            LocalAgentKind::Gemini => "gemini",
        }
    }

    /// What a person calls it.
    pub fn label(self) -> &'static str {
        match self {
            LocalAgentKind::ClaudeCode => "Claude Code",
            LocalAgentKind::Codex => "Codex",
            LocalAgentKind::Gemini => "Gemini CLI",
        }
    }

    /// Whose computers the prompt is actually sent to.
    pub fn vendor(self) -> &'static str {
        match self {
            LocalAgentKind::ClaudeCode => "Anthropic",
            LocalAgentKind::Codex => "OpenAI",
            LocalAgentKind::Gemini => "Google",
        }
    }

    /// Environment variables removed before the CLI starts.
    ///
    /// Each would switch the CLI from the person's subscription sign-in to
    /// metered API billing without telling anyone, which is the exact cost
    /// this feature exists to avoid.
    pub fn scrubbed_env(self) -> &'static [&'static str] {
        match self {
            LocalAgentKind::ClaudeCode => &[
                "ANTHROPIC_API_KEY",
                "ANTHROPIC_AUTH_TOKEN",
                "ANTHROPIC_BASE_URL",
                "CLAUDE_CODE_USE_BEDROCK",
                "CLAUDE_CODE_USE_VERTEX",
                "CLAUDE_CODE_USE_FOUNDRY",
            ],
            LocalAgentKind::Codex => &["OPENAI_API_KEY", "CODEX_API_KEY", "OPENAI_BASE_URL"],
            LocalAgentKind::Gemini => &["GEMINI_API_KEY", "GOOGLE_API_KEY"],
        }
    }

    /// Arguments for one non-interactive turn, prompt on stdin.
    ///
    /// Every agent's own tools are turned off or confined: it reads nothing,
    /// runs nothing and edits nothing. The vehicle is reached only through the
    /// tools the application offers in the prompt, which go through the same
    /// safety gate as any other model's. The working folder is an empty one
    /// made for this call.
    ///
    /// `output_file` is where Codex writes its final message.
    pub fn args(self, model: Option<&str>, work_dir: &Path, output_file: &Path) -> Vec<String> {
        let mut a: Vec<String> = match self {
            LocalAgentKind::ClaudeCode => [
                "-p",
                "--output-format",
                "json",
                "--tools",
                "",
                "--safe-mode",
                "--strict-mcp-config",
                "--no-session-persistence",
                "--permission-mode",
                "dontAsk",
                "--append-system-prompt",
                "You are answering inside WTFault Scanner, a vehicle diagnostic app. Follow the \
                 instructions in the message exactly. You have no tools of your own.",
            ]
            .iter()
            .map(|s| s.to_string())
            .collect(),
            LocalAgentKind::Codex => vec![
                "exec".into(),
                "--sandbox".into(),
                "read-only".into(),
                "--skip-git-repo-check".into(),
                "--ephemeral".into(),
                "--color".into(),
                "never".into(),
                "-C".into(),
                work_dir.display().to_string(),
                "-o".into(),
                output_file.display().to_string(),
            ],
            LocalAgentKind::Gemini => vec!["--approval-mode".into(), "plan".into()],
        };
        if let Some(m) = model.map(str::trim).filter(|m| !m.is_empty() && *m != "default") {
            a.push(if self == LocalAgentKind::ClaudeCode { "--model" } else { "-m" }.into());
            a.push(m.to_string());
        }
        match self {
            // `-` reads the prompt from stdin.
            LocalAgentKind::Codex => a.push("-".into()),
            // `-p` is required for headless mode and is appended to stdin.
            LocalAgentKind::Gemini => {
                a.push("-p".into());
                a.push(" ".into());
            }
            LocalAgentKind::ClaudeCode => {}
        }
        a
    }

    /// A cheap command that says whether the CLI is signed in, if it has one.
    pub fn sign_in_check(self) -> Option<&'static [&'static str]> {
        match self {
            LocalAgentKind::ClaudeCode => Some(&["auth", "status"]),
            LocalAgentKind::Codex => Some(&["login", "status"]),
            LocalAgentKind::Gemini => None,
        }
    }
}

/// Read the sign-in check's output.
///
/// `claude auth status` prints JSON and exits 1 when signed out. `codex login
/// status` prints a sentence. Anything unrecognised is `None`, never a guess.
pub fn interpret_sign_in(
    kind: LocalAgentKind,
    ok: bool,
    stdout: &str,
    stderr: &str,
) -> Option<bool> {
    match kind {
        LocalAgentKind::ClaudeCode => {
            let v: serde_json::Value = serde_json::from_str(stdout.trim()).ok()?;
            v.get("loggedIn").and_then(|b| b.as_bool())
        }
        LocalAgentKind::Codex => {
            let all = format!("{stdout}\n{stderr}").to_ascii_lowercase();
            if all.contains("not logged in") {
                Some(false)
            } else if ok && all.contains("logged in") {
                Some(true)
            } else {
                None
            }
        }
        LocalAgentKind::Gemini => None,
    }
}

// ------------------------------------------------------------- discovery

/// Where an installer may have put the CLI, beyond `PATH`.
///
/// Takes its inputs rather than reading the process environment, so it can be
/// tested for Windows paths on any machine.
pub fn candidate_paths(
    kind: LocalAgentKind,
    path_var: &str,
    home: Option<&Path>,
    windows: bool,
    env: &dyn Fn(&str) -> Option<String>,
) -> Vec<PathBuf> {
    let name = kind.command();
    let names: Vec<String> = if windows {
        // npm puts a `.cmd` shim on PATH; native installers put an `.exe`.
        let pathext = env("PATHEXT").unwrap_or_else(|| ".EXE;.CMD;.BAT".into());
        let mut exts: Vec<String> =
            pathext.split(';').filter(|e| !e.is_empty()).map(|e| e.to_ascii_lowercase()).collect();
        exts.retain(|e| matches!(e.as_str(), ".exe" | ".cmd" | ".bat"));
        if exts.is_empty() {
            exts = vec![".exe".into(), ".cmd".into()];
        }
        exts.iter().map(|e| format!("{name}{e}")).collect()
    } else {
        vec![name.to_string()]
    };

    let mut dirs: Vec<PathBuf> = Vec::new();
    let sep = if windows { ';' } else { ':' };
    dirs.extend(path_var.split(sep).filter(|d| !d.trim().is_empty()).map(PathBuf::from));

    if let Some(home) = home {
        dirs.push(home.join(".local").join("bin"));
        dirs.push(home.join(".npm-global").join("bin"));
        if !windows {
            // A GUI app on macOS does not inherit the shell's PATH.
            dirs.push(PathBuf::from("/opt/homebrew/bin"));
            dirs.push(PathBuf::from("/usr/local/bin"));
            dirs.push(home.join(".bun").join("bin"));
        }
    }
    let mut found: Vec<PathBuf> = Vec::new();
    if windows {
        let app_data = env("APPDATA")
            .map(PathBuf::from)
            .or_else(|| home.map(|h| h.join("AppData").join("Roaming")));
        let local = env("LOCALAPPDATA")
            .map(PathBuf::from)
            .or_else(|| home.map(|h| h.join("AppData").join("Local")));
        if let Some(app_data) = &app_data {
            dirs.push(app_data.join("npm"));
            match kind {
                LocalAgentKind::ClaudeCode => {
                    found.push(
                        app_data.join("npm/node_modules/@anthropic-ai/claude-code/bin/claude.exe"),
                    );
                    // Claude desktop keeps its own copy in claude-code\<version>[\<build>].
                    found.extend(expand(app_data, &["Claude", "claude-code", "*", "claude.exe"]));
                    found.extend(expand(
                        app_data,
                        &["Claude", "claude-code", "*", "*", "claude.exe"],
                    ));
                }
                LocalAgentKind::Codex => found.push(app_data.join(
                    "npm/node_modules/@openai/codex/node_modules/@openai/codex-win32-x64/vendor/x86_64-pc-windows-msvc/codex/codex.exe",
                )),
                LocalAgentKind::Gemini => {}
            }
        }
        if let Some(local) = &local {
            match kind {
                LocalAgentKind::ClaudeCode => {
                    found.extend(expand(
                        local,
                        &["Claude-Data", "*", "claude-code", "*", "*", "claude.exe"],
                    ));
                    // An MSIX package sees its Roaming folder under LocalCache.
                    found.extend(expand(
                        local,
                        &[
                            "Packages",
                            "Claude_*",
                            "LocalCache",
                            "Roaming",
                            "Claude",
                            "claude-code",
                            "*",
                            "*",
                            "claude.exe",
                        ],
                    ));
                }
                LocalAgentKind::Codex => {
                    found.extend(expand(local, &["OpenAI", "Codex", "bin", "*", "codex.exe"]))
                }
                LocalAgentKind::Gemini => {}
            }
        }
    }
    for d in dirs {
        for n in &names {
            found.push(d.join(n));
        }
    }
    let mut seen = std::collections::HashSet::new();
    found.retain(|p| seen.insert(p.clone()));
    found
}

/// Join `segments` under `base`, where a segment `*` or `prefix*` matches every
/// child folder starting with `prefix`. Versioned install folders need it.
fn expand(base: &Path, segments: &[&str]) -> Vec<PathBuf> {
    let mut paths = vec![base.to_path_buf()];
    for seg in segments {
        match seg.strip_suffix('*') {
            None => paths = paths.into_iter().map(|p| p.join(seg)).collect(),
            Some(prefix) => {
                let prefix = prefix.to_ascii_lowercase();
                paths = paths
                    .into_iter()
                    .flat_map(|p| {
                        std::fs::read_dir(&p)
                            .into_iter()
                            .flatten()
                            .flatten()
                            .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
                            .filter(|e| {
                                e.file_name()
                                    .to_string_lossy()
                                    .to_ascii_lowercase()
                                    .starts_with(&prefix)
                            })
                            .map(|e| e.path())
                            .collect::<Vec<_>>()
                    })
                    .collect();
            }
        }
    }
    paths
}

/// `(major, minor, patch)` from the first version-looking run in `text`.
pub fn version_of(text: &str) -> Option<(u64, u64, u64)> {
    let bytes = text.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i].is_ascii_digit() {
            let rest = &text[i..];
            let token: String =
                rest.chars().take_while(|c| c.is_ascii_digit() || *c == '.').collect();
            let parts: Vec<u64> = token.split('.').filter_map(|p| p.parse().ok()).collect();
            if parts.len() >= 3 {
                return Some((parts[0], parts[1], parts[2]));
            }
            i += token.len().max(1);
        } else {
            i += 1;
        }
    }
    None
}

/// An installed agent that was found.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DetectedAgent {
    /// Which agent.
    pub kind: LocalAgentKind,
    /// Its name, for the interface.
    pub label: String,
    /// The executable that will be run.
    pub path: String,
    /// What `--version` printed, trimmed.
    pub version: Option<String>,
    /// Whether it says it is signed in. `None` when it cannot tell us cheaply.
    pub signed_in: Option<bool>,
    /// Whose computers it sends prompts to.
    pub vendor: String,
}

/// Find the newest copy of `kind`, or use `override_path` when given.
pub async fn locate(
    kind: LocalAgentKind,
    override_path: Option<&str>,
    runner: &dyn CliRunner,
) -> Option<(PathBuf, Option<String>)> {
    if let Some(p) = override_path.map(str::trim).filter(|p| !p.is_empty()) {
        let p = PathBuf::from(p);
        return p.is_file().then_some((p, None));
    }
    let home = home_dir();
    let path_var = std::env::var("PATH").or_else(|_| std::env::var("Path")).unwrap_or_default();
    let candidates = candidate_paths(kind, &path_var, home.as_deref(), cfg!(windows), &|k| {
        std::env::var(k).ok()
    });
    let present: Vec<PathBuf> = candidates.into_iter().filter(|p| p.is_file()).collect();
    if present.is_empty() {
        return None;
    }
    // The path, what `--version` printed, and that read as a version.
    type Candidate = (PathBuf, Option<String>, Option<(u64, u64, u64)>);
    let mut best: Option<Candidate> = None;
    for p in present {
        let out = runner
            .run(Invocation::probe(p.clone(), &["--version"], kind))
            .await
            .ok()
            .filter(|f| f.success());
        let text = out.map(|f| f.stdout.trim().to_string()).filter(|s| !s.is_empty());
        let v = text.as_deref().and_then(version_of);
        let better = match &best {
            None => true,
            Some((_, _, bv)) => v > *bv,
        };
        if better {
            best = Some((p, text, v));
        }
    }
    best.map(|(p, t, _)| (p, t))
}

/// Everything installed, with versions and sign-in where it can be told.
pub async fn detect_all(runner: &dyn CliRunner) -> Vec<DetectedAgent> {
    let mut out = Vec::new();
    for kind in LocalAgentKind::ALL {
        let Some((path, version)) = locate(kind, None, runner).await else { continue };
        let signed_in = match kind.sign_in_check() {
            Some(args) => match runner.run(Invocation::probe(path.clone(), args, kind)).await {
                Ok(f) => interpret_sign_in(kind, f.success(), &f.stdout, &f.stderr),
                Err(_) => None,
            },
            None => None,
        };
        out.push(DetectedAgent {
            kind,
            label: kind.label().to_string(),
            path: path.display().to_string(),
            version,
            signed_in,
            vendor: kind.vendor().to_string(),
        });
    }
    out
}

fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .filter(|p| !p.as_os_str().is_empty())
}

// ---------------------------------------------------------------- running

/// One run of a CLI.
#[derive(Debug, Clone)]
pub struct Invocation {
    /// The executable.
    pub program: PathBuf,
    /// Its arguments.
    pub args: Vec<String>,
    /// Written to stdin, then stdin is closed.
    pub stdin: String,
    /// Working directory. `None` keeps the current one.
    pub cwd: Option<PathBuf>,
    /// Variables removed from the child's environment.
    pub remove_env: Vec<String>,
    /// Killed after this long.
    pub timeout: Duration,
}

impl Invocation {
    /// A short informational call such as `--version`.
    pub fn probe(program: PathBuf, args: &[&str], kind: LocalAgentKind) -> Self {
        Invocation {
            program,
            args: args.iter().map(|s| s.to_string()).collect(),
            stdin: String::new(),
            cwd: None,
            remove_env: kind.scrubbed_env().iter().map(|s| s.to_string()).collect(),
            timeout: Duration::from_secs(15),
        }
    }
}

/// How a run ended.
#[derive(Debug, Clone, Default)]
pub struct Finished {
    /// Exit code, when it exited rather than being killed.
    pub code: Option<i32>,
    /// Standard output.
    pub stdout: String,
    /// Standard error, trimmed to its tail.
    pub stderr: String,
    /// Whether it was killed for running past its timeout.
    pub timed_out: bool,
}

impl Finished {
    /// Exited zero.
    pub fn success(&self) -> bool {
        self.code == Some(0) && !self.timed_out
    }
}

/// Runs a CLI. The real one spawns a process; tests supply their own.
#[async_trait::async_trait]
pub trait CliRunner: Send + Sync {
    /// Run to completion or timeout. `Err` only when it could not be started.
    async fn run(&self, invocation: Invocation) -> std::io::Result<Finished>;
}

/// Spawns the process, with no console window on Windows.
pub struct ProcessRunner;

#[async_trait::async_trait]
impl CliRunner for ProcessRunner {
    async fn run(&self, inv: Invocation) -> std::io::Result<Finished> {
        use tokio::io::AsyncWriteExt;
        let mut cmd = tokio::process::Command::new(&inv.program);
        cmd.args(&inv.args)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true);
        for k in &inv.remove_env {
            cmd.env_remove(k);
        }
        if let Some(dir) = &inv.cwd {
            cmd.current_dir(dir);
        }
        #[cfg(windows)]
        {
            // CREATE_NO_WINDOW: a console app started from a GUI otherwise
            // flashes a black window for every model turn.
            cmd.creation_flags(0x0800_0000);
        }
        let mut child = cmd.spawn()?;
        let pid = child.id();
        if let Some(mut stdin) = child.stdin.take() {
            // A CLI that exits before reading everything closes the pipe;
            // that is reported through its exit status, not here.
            let _ = stdin.write_all(inv.stdin.as_bytes()).await;
            let _ = stdin.shutdown().await;
        }
        match tokio::time::timeout(inv.timeout, child.wait_with_output()).await {
            Ok(out) => {
                let out = out?;
                Ok(Finished {
                    code: out.status.code(),
                    stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
                    stderr: tail(&String::from_utf8_lossy(&out.stderr), 4000),
                    timed_out: false,
                })
            }
            Err(_) => {
                // The future owning the child is dropped, which kills it
                // (`kill_on_drop`). On Windows its own children survive that,
                // so the whole tree is ended too.
                #[cfg(windows)]
                if let Some(pid) = pid {
                    let _ = std::process::Command::new("taskkill.exe")
                        .args(["/PID", &pid.to_string(), "/T", "/F"])
                        .stdout(std::process::Stdio::null())
                        .stderr(std::process::Stdio::null())
                        .status();
                }
                let _ = pid;
                Ok(Finished { timed_out: true, ..Default::default() })
            }
        }
    }
}

fn tail(s: &str, max: usize) -> String {
    let count = s.chars().count();
    if count <= max {
        s.to_string()
    } else {
        s.chars().skip(count - max).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn claude_runs_with_its_own_tools_off_and_prompt_on_stdin() {
        let a = LocalAgentKind::ClaudeCode.args(None, Path::new("/w"), Path::new("/w/out"));
        let tools = a.iter().position(|s| s == "--tools").unwrap();
        assert_eq!(a[tools + 1], "", "an empty tool list turns them all off");
        for flag in ["-p", "--safe-mode", "--strict-mcp-config", "--no-session-persistence"] {
            assert!(a.iter().any(|s| s == flag), "{flag}");
        }
        assert!(!a.iter().any(|s| s == "--model"), "blank model is the CLI's default");
        let a = LocalAgentKind::ClaudeCode.args(Some("sonnet"), Path::new("/w"), Path::new("/o"));
        assert!(a.windows(2).any(|w| w == ["--model", "sonnet"]));
    }

    #[test]
    fn codex_is_read_only_in_the_empty_folder_and_writes_its_answer_to_a_file() {
        let a = LocalAgentKind::Codex.args(Some("gpt-5"), Path::new("/w"), Path::new("/w/out"));
        assert_eq!(a[0], "exec");
        assert!(a.windows(2).any(|w| w == ["--sandbox", "read-only"]));
        assert!(a.windows(2).any(|w| w == ["-C", "/w"]));
        assert!(a.windows(2).any(|w| w == ["-o", "/w/out"]));
        assert!(a.windows(2).any(|w| w == ["-m", "gpt-5"]));
        assert_eq!(a.last().unwrap(), "-", "prompt from stdin");
    }

    #[test]
    fn api_keys_never_reach_the_child() {
        assert!(LocalAgentKind::ClaudeCode.scrubbed_env().contains(&"ANTHROPIC_API_KEY"));
        assert!(LocalAgentKind::Codex.scrubbed_env().contains(&"OPENAI_API_KEY"));
        let inv =
            Invocation::probe(PathBuf::from("claude"), &["--version"], LocalAgentKind::ClaudeCode);
        assert!(inv.remove_env.iter().any(|k| k == "ANTHROPIC_API_KEY"));
    }

    #[test]
    fn windows_finds_npm_shims_and_desktop_copies() {
        let env = |k: &str| match k {
            "APPDATA" => Some(r"C:\Users\S\AppData\Roaming".to_string()),
            "LOCALAPPDATA" => Some(r"C:\Users\S\AppData\Local".to_string()),
            "PATHEXT" => Some(".COM;.EXE;.BAT;.CMD".to_string()),
            _ => None,
        };
        let c = candidate_paths(
            LocalAgentKind::ClaudeCode,
            r"C:\tools;C:\bin",
            Some(Path::new(r"C:\Users\S")),
            true,
            &env,
        );
        let shown: Vec<String> = c.iter().map(|p| p.display().to_string()).collect();
        assert!(shown.iter().any(|s| s.ends_with("claude.cmd")), "{shown:?}");
        assert!(shown.iter().any(|s| s.ends_with("claude.exe")), "{shown:?}");
        assert!(shown.iter().any(|s| s.contains("@anthropic-ai")), "{shown:?}");
        assert!(!shown.iter().any(|s| s.ends_with("claude.com")), "only runnable extensions");
    }

    #[test]
    fn unix_adds_the_folders_a_gui_app_does_not_inherit() {
        let c = candidate_paths(
            LocalAgentKind::Codex,
            "/usr/bin",
            Some(Path::new("/home/c")),
            false,
            &|_| None,
        );
        assert!(c.contains(&PathBuf::from("/usr/bin/codex")));
        assert!(c.contains(&PathBuf::from("/home/c/.local/bin/codex")));
        assert!(c.contains(&PathBuf::from("/opt/homebrew/bin/codex")));
    }

    #[test]
    fn versions_compare_as_numbers() {
        assert_eq!(version_of("2.1.331 (Claude Code)"), Some((2, 1, 331)));
        assert_eq!(version_of("codex-cli 0.171.0"), Some((0, 171, 0)));
        assert!(version_of("2.10.0") > version_of("2.9.9"));
        assert_eq!(version_of("no version"), None);
    }

    #[test]
    fn sign_in_is_read_only_when_it_is_clear() {
        let k = LocalAgentKind::ClaudeCode;
        assert_eq!(interpret_sign_in(k, false, r#"{"loggedIn":false}"#, ""), Some(false));
        assert_eq!(
            interpret_sign_in(k, true, r#"{"loggedIn":true,"authMethod":"claude.ai"}"#, ""),
            Some(true)
        );
        assert_eq!(interpret_sign_in(k, true, "garbage", ""), None);
        let k = LocalAgentKind::Codex;
        assert_eq!(interpret_sign_in(k, true, "Logged in using ChatGPT", ""), Some(true));
        assert_eq!(interpret_sign_in(k, false, "", "Not logged in"), Some(false));
    }

    /// The real runner, against a command every Unix machine has. Proves the
    /// stdin, environment scrubbing and timeout plumbing without any agent.
    #[cfg(unix)]
    #[tokio::test]
    async fn the_process_runner_pipes_stdin_scrubs_env_and_times_out() {
        std::env::set_var("WTF_TEST_SECRET", "leak");
        let f = ProcessRunner
            .run(Invocation {
                program: PathBuf::from("/bin/sh"),
                args: vec!["-c".into(), "cat; echo \"[${WTF_TEST_SECRET:-none}]\"".into()],
                stdin: "hello ".into(),
                cwd: None,
                remove_env: vec!["WTF_TEST_SECRET".into()],
                timeout: Duration::from_secs(10),
            })
            .await
            .unwrap();
        assert!(f.success());
        assert_eq!(f.stdout.trim(), "hello [none]");

        let f = ProcessRunner
            .run(Invocation {
                program: PathBuf::from("/bin/sh"),
                args: vec!["-c".into(), "sleep 5".into()],
                stdin: String::new(),
                cwd: None,
                remove_env: vec![],
                timeout: Duration::from_millis(200),
            })
            .await
            .unwrap();
        assert!(f.timed_out && !f.success());
    }
}
