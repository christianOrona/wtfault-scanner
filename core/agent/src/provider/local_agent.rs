//! An installed agent CLI as a model provider.
//!
//! Each [`LlmProvider::chat`] call is one non-interactive run of the CLI: the
//! whole request written as one prompt on stdin (see [`super::text_tools`]),
//! the reply read back into the same content blocks every other provider
//! produces. The agent loop, the safety gate and the claim checks cannot tell
//! the difference, which is the point.

use super::text_tools::{self, Parsed};
use super::{
    ChatRequest, ChatResponse, Content, LlmProvider, Message, ProviderInfo, StopReason, Usage,
};
use crate::error::AgentError;
use crate::local_agents::{self, CliRunner, Finished, Invocation, LocalAgentKind, ProcessRunner};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// How long one turn may take. A signed-in CLI answers in seconds to a minute;
/// five covers a slow reasoning model without leaving a hung one forever.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(300);

/// A configured local agent.
pub struct LocalAgentProvider {
    id: String,
    label: String,
    model: String,
    kind: LocalAgentKind,
    executable: Option<String>,
    timeout: Duration,
    runner: Arc<dyn CliRunner>,
    calls: AtomicU64,
}

impl LocalAgentProvider {
    /// A provider that spawns the real CLI.
    pub fn new(
        id: &str,
        label: &str,
        model: &str,
        kind: LocalAgentKind,
        executable: Option<&str>,
    ) -> Self {
        Self::with_runner(id, label, model, kind, executable, Arc::new(ProcessRunner))
    }

    /// The same, running through `runner`. Tests use this.
    pub fn with_runner(
        id: &str,
        label: &str,
        model: &str,
        kind: LocalAgentKind,
        executable: Option<&str>,
        runner: Arc<dyn CliRunner>,
    ) -> Self {
        LocalAgentProvider {
            id: id.into(),
            label: label.into(),
            model: model.into(),
            kind,
            executable: executable.map(String::from).filter(|s| !s.trim().is_empty()),
            timeout: DEFAULT_TIMEOUT,
            runner,
            calls: AtomicU64::new(0),
        }
    }

    /// Override the per-turn timeout.
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    fn fail(&self, detail: impl Into<String>) -> AgentError {
        AgentError::LocalAgent { provider: self.label.clone(), detail: detail.into() }
    }

    async fn program(&self) -> Result<PathBuf, AgentError> {
        match local_agents::locate(self.kind, self.executable.as_deref(), self.runner.as_ref())
            .await
        {
            Some((p, _)) => Ok(p),
            None => Err(self.fail(match &self.executable {
                Some(p) => format!("no executable at {p}. Check the path in Settings."),
                None => format!(
                    "{} is not installed, or it is somewhere this app does not look. Install it \
                     and sign in, or set the full path to it in Settings.",
                    self.kind.label()
                ),
            })),
        }
    }

    /// Run the CLI once and return the text of its reply.
    async fn ask(&self, prompt: String) -> Result<(String, Usage), AgentError> {
        let program = self.program().await?;
        let n = self.calls.fetch_add(1, Ordering::Relaxed);
        let dir = ScratchDir::new(&self.id, n)
            .map_err(|e| self.fail(format!("no temporary folder: {e}")))?;
        let output_file = dir.path.join("answer.txt");
        let model = Some(self.model.as_str());
        let invocation = Invocation {
            program,
            args: self.kind.args(model, &dir.path, &output_file),
            stdin: prompt,
            cwd: Some(dir.path.clone()),
            remove_env: self.kind.scrubbed_env().iter().map(|s| s.to_string()).collect(),
            timeout: self.timeout,
        };
        let finished = self
            .runner
            .run(invocation)
            .await
            .map_err(|e| self.fail(format!("could not be started: {e}")))?;
        let from_file = std::fs::read_to_string(&output_file).ok();
        self.read_output(finished, from_file)
    }

    fn read_output(
        &self,
        f: Finished,
        from_file: Option<String>,
    ) -> Result<(String, Usage), AgentError> {
        if f.timed_out {
            return Err(self.fail(format!(
                "did not answer within {} seconds and was stopped",
                self.timeout.as_secs()
            )));
        }
        match self.kind {
            LocalAgentKind::ClaudeCode => {
                // `--output-format json`: one result object, also on failure.
                let v: Option<serde_json::Value> = serde_json::from_str(f.stdout.trim()).ok();
                let Some(v) = v else {
                    return Err(
                        self.fail(explain(&f, "returned something that is not its JSON result"))
                    );
                };
                let text = v.get("result").and_then(|r| r.as_str()).unwrap_or("").to_string();
                let is_error = v.get("is_error").and_then(|b| b.as_bool()).unwrap_or(false)
                    || v.get("subtype").and_then(|s| s.as_str()).is_some_and(|s| s != "success");
                if is_error || !f.success() {
                    let said = if text.trim().is_empty() { explain(&f, "failed") } else { text };
                    return Err(self.fail(classify(&said)));
                }
                let u = v.get("usage");
                let n = |k: &str| u.and_then(|u| u.get(k)).and_then(|x| x.as_u64()).unwrap_or(0);
                Ok((
                    text,
                    Usage {
                        input_tokens: n("input_tokens") + n("cache_creation_input_tokens"),
                        output_tokens: n("output_tokens"),
                        cache_read_tokens: n("cache_read_input_tokens"),
                    },
                ))
            }
            LocalAgentKind::Codex | LocalAgentKind::Gemini => {
                if !f.success() {
                    return Err(self.fail(classify(&explain(&f, "failed"))));
                }
                let text =
                    from_file.filter(|t| !t.trim().is_empty()).unwrap_or_else(|| f.stdout.clone());
                if text.trim().is_empty() {
                    return Err(self.fail(explain(&f, "finished without an answer")));
                }
                Ok((text, Usage::default()))
            }
        }
    }
}

/// The CLI's own words when it has some, else `fallback`.
fn explain(f: &Finished, fallback: &str) -> String {
    let said = if f.stderr.trim().is_empty() { f.stdout.trim() } else { f.stderr.trim() };
    if said.is_empty() {
        format!("{fallback} (exit {})", f.code.map(|c| c.to_string()).unwrap_or_else(|| "?".into()))
    } else {
        said.chars().take(600).collect()
    }
}

/// Put the remedy in front when the failure is one a person can fix.
fn classify(said: &str) -> String {
    let lower = said.to_ascii_lowercase();
    if ["not logged in", "login", "log in", "sign in", "authenticat", "unauthorized"]
        .iter()
        .any(|k| lower.contains(k))
    {
        format!("not signed in. Open a terminal, sign in to the CLI, then try again. ({said})")
    } else if ["usage limit", "rate limit", "quota", "limit reached", "hit your"]
        .iter()
        .any(|k| lower.contains(k))
    {
        format!("out of allowance for now. ({said})")
    } else {
        said.to_string()
    }
}

#[async_trait::async_trait]
impl LlmProvider for LocalAgentProvider {
    fn id(&self) -> &str {
        &self.id
    }
    fn label(&self) -> &str {
        &self.label
    }
    fn model(&self) -> &str {
        if self.model.trim().is_empty() {
            "default"
        } else {
            &self.model
        }
    }

    async fn chat(&self, request: &ChatRequest) -> Result<ChatResponse, AgentError> {
        let step = self.calls.load(Ordering::Relaxed);
        let (text, mut usage) = self.ask(text_tools::render_prompt(request)).await?;
        let content = match text_tools::parse_reply(&text, &format!("call{step}")) {
            Parsed::Ok(c) => c,
            Parsed::Malformed { text: prose, error } if !request.tools.is_empty() => {
                // Handed back once with the reason, like a schema error.
                let mut retry = request.clone();
                retry.messages.push(Message::assistant(text.trim()));
                retry.messages.push(Message::user(text_tools::malformed_feedback(&error)));
                let (again, u2) = self.ask(text_tools::render_prompt(&retry)).await?;
                usage.input_tokens += u2.input_tokens;
                usage.output_tokens += u2.output_tokens;
                usage.cache_read_tokens += u2.cache_read_tokens;
                match text_tools::parse_reply(&again, &format!("call{step}r")) {
                    Parsed::Ok(c) => c,
                    // Twice is enough: show what it said as prose.
                    Parsed::Malformed { .. } => {
                        let shown = if prose.is_empty() { again.trim().to_string() } else { prose };
                        vec![Content::text(shown)]
                    }
                }
            }
            Parsed::Malformed { text, .. } => vec![Content::text(text)],
        };
        let wants_tools = content.iter().any(|c| matches!(c, Content::ToolUse { .. }));
        Ok(ChatResponse {
            model: format!("{} ({})", self.kind.label(), self.model()),
            content,
            stop_reason: if wants_tools { StopReason::ToolUse } else { StopReason::EndTurn },
            usage,
        })
    }

    async fn probe(&self) -> Result<ProviderInfo, AgentError> {
        let started = Instant::now();
        let program = self.program().await?;
        let version = self
            .runner
            .run(Invocation::probe(program.clone(), &["--version"], self.kind))
            .await
            .ok()
            .filter(|f| f.success())
            .map(|f| f.stdout.trim().to_string());
        let signed_in = match self.kind.sign_in_check() {
            Some(args) => match self
                .runner
                .run(Invocation::probe(program.clone(), args, self.kind))
                .await
            {
                Ok(f) => {
                    local_agents::interpret_sign_in(self.kind, f.success(), &f.stdout, &f.stderr)
                }
                Err(_) => None,
            },
            None => None,
        };
        let mut detail = format!(
            "{} at {}{}. Prompts go to {} on your own sign-in, not an API key.",
            self.kind.label(),
            program.display(),
            version.map(|v| format!(", version {v}")).unwrap_or_default(),
            self.kind.vendor()
        );
        match signed_in {
            Some(false) => {
                return Err(self.fail(format!(
                "installed at {} but not signed in. Run `{} {}` in a terminal, then test again.",
                program.display(),
                program.display(),
                if self.kind == LocalAgentKind::Codex { "login" } else { "auth login" }
            )))
            }
            Some(true) => detail.push_str(" Signed in."),
            None => detail.push_str(" Sign-in could not be checked without spending a turn."),
        }
        Ok(ProviderInfo {
            reachable: true,
            models: Vec::new(),
            elapsed_ms: started.elapsed().as_millis() as u64,
            detail: Some(detail),
        })
    }
}

/// An empty folder for one run, removed afterwards.
///
/// The CLI is started inside it so that even a confined agent has nothing to
/// look at: not this app's data, not the person's home folder.
struct ScratchDir {
    path: PathBuf,
}

impl ScratchDir {
    fn new(id: &str, n: u64) -> std::io::Result<Self> {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let safe: String = id.chars().filter(|c| c.is_ascii_alphanumeric() || *c == '_').collect();
        let path = std::env::temp_dir()
            .join(format!("wtfault-agent-{safe}-{}-{n}-{nanos}", std::process::id()));
        std::fs::create_dir_all(&path)?;
        Ok(ScratchDir { path })
    }
}

impl Drop for ScratchDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::{Role, ToolSpec};
    use serde_json::json;
    use std::sync::Mutex;

    /// Answers each run from a script and records what it was asked.
    struct FakeCli {
        replies: Mutex<Vec<Finished>>,
        seen: Mutex<Vec<Invocation>>,
    }

    impl FakeCli {
        fn new(replies: Vec<Finished>) -> Arc<Self> {
            Arc::new(FakeCli { replies: Mutex::new(replies), seen: Mutex::new(vec![]) })
        }
    }

    #[async_trait::async_trait]
    impl CliRunner for FakeCli {
        async fn run(&self, inv: Invocation) -> std::io::Result<Finished> {
            let mut r = self.replies.lock().unwrap();
            self.seen.lock().unwrap().push(inv.clone());
            if inv.args.first().map(String::as_str) == Some("--version") {
                return Ok(Finished {
                    code: Some(0),
                    stdout: "9.9.9".into(),
                    ..Default::default()
                });
            }
            Ok(if r.is_empty() { Finished::default() } else { r.remove(0) })
        }
    }

    fn claude_json(result: &str) -> Finished {
        Finished {
            code: Some(0),
            stdout: json!({
                "type": "result", "subtype": "success", "is_error": false, "result": result,
                "usage": {"input_tokens": 10, "output_tokens": 5, "cache_read_input_tokens": 2}
            })
            .to_string(),
            ..Default::default()
        }
    }

    fn provider(kind: LocalAgentKind, cli: Arc<FakeCli>) -> LocalAgentProvider {
        // Any file that exists stands in for the executable.
        let exe = std::env::current_exe().unwrap();
        LocalAgentProvider::with_runner("p1", "Mine", "", kind, exe.to_str(), cli)
    }

    fn request(tools: bool) -> ChatRequest {
        ChatRequest {
            system: "You are a mechanic.".into(),
            messages: vec![Message::user("Any codes?")],
            tools: if tools {
                vec![ToolSpec {
                    name: "read_dtcs".into(),
                    description: "Read codes.".into(),
                    input_schema: json!({"type": "object"}),
                }]
            } else {
                vec![]
            },
            max_tokens: 1000,
        }
    }

    #[tokio::test]
    async fn a_claude_reply_with_a_block_becomes_a_tool_call() {
        let cli = FakeCli::new(vec![claude_json(
            "Checking.\n<tool_calls>[{\"name\":\"read_dtcs\",\"input\":{}}]</tool_calls>",
        )]);
        let p = provider(LocalAgentKind::ClaudeCode, cli.clone());
        let r = p.chat(&request(true)).await.unwrap();
        assert_eq!(r.stop_reason, StopReason::ToolUse);
        assert_eq!(r.tool_calls()[0].1, "read_dtcs");
        assert_eq!(r.usage.output_tokens, 5);
        let seen = cli.seen.lock().unwrap();
        let run = seen.iter().find(|i| i.args.first().map(String::as_str) == Some("-p")).unwrap();
        assert!(run.stdin.contains("You are a mechanic."), "the prompt goes on stdin");
        assert!(run.remove_env.iter().any(|k| k == "ANTHROPIC_API_KEY"));
    }

    #[tokio::test]
    async fn a_malformed_block_is_handed_back_once() {
        let cli = FakeCli::new(vec![
            claude_json("<tool_calls>[{oops}]</tool_calls>"),
            claude_json("No codes are stored."),
        ]);
        let p = provider(LocalAgentKind::ClaudeCode, cli.clone());
        let r = p.chat(&request(true)).await.unwrap();
        assert_eq!(r.text(), "No codes are stored.");
        let seen = cli.seen.lock().unwrap();
        let second = seen
            .iter()
            .filter(|i| i.args.first().map(String::as_str) == Some("-p"))
            .nth(1)
            .unwrap();
        assert!(second.stdin.contains("could not be used"), "{}", second.stdin);
    }

    #[tokio::test]
    async fn codex_answers_from_its_output_file_or_stdout() {
        let cli = FakeCli::new(vec![Finished {
            code: Some(0),
            stdout: "The catalyst is borderline.".into(),
            ..Default::default()
        }]);
        let p = provider(LocalAgentKind::Codex, cli);
        let r = p.chat(&request(false)).await.unwrap();
        assert_eq!(r.text(), "The catalyst is borderline.");
        assert_eq!(r.content[0], Content::text("The catalyst is borderline."));
        assert!(matches!(r.stop_reason, StopReason::EndTurn));
        let _ = Role::User;
    }

    #[tokio::test]
    async fn a_signed_out_cli_says_how_to_fix_it() {
        let cli = FakeCli::new(vec![Finished {
            code: Some(1),
            stdout: json!({"type":"result","subtype":"success","is_error":true,
                           "result":"Not logged in · Please run /login"})
            .to_string(),
            ..Default::default()
        }]);
        let p = provider(LocalAgentKind::ClaudeCode, cli);
        let e = p.chat(&request(false)).await.unwrap_err();
        assert_eq!(e.code(), "local_agent_failed");
        assert!(e.to_string().contains("not signed in"), "{e}");
    }

    #[tokio::test]
    async fn a_timeout_is_reported_not_hung() {
        let cli = FakeCli::new(vec![Finished { timed_out: true, ..Default::default() }]);
        let p = provider(LocalAgentKind::Codex, cli);
        let e = p.chat(&request(false)).await.unwrap_err();
        assert!(e.to_string().contains("did not answer"), "{e}");
    }

    #[tokio::test]
    async fn a_missing_executable_names_the_fix() {
        let cli = FakeCli::new(vec![]);
        let p = LocalAgentProvider::with_runner(
            "p",
            "Mine",
            "",
            LocalAgentKind::Codex,
            Some("/nowhere/codex"),
            cli,
        );
        let e = p.chat(&request(false)).await.unwrap_err();
        assert!(e.to_string().contains("Check the path in Settings"), "{e}");
    }

    #[tokio::test]
    async fn probe_reports_a_signed_out_claude() {
        struct SignedOut;
        #[async_trait::async_trait]
        impl CliRunner for SignedOut {
            async fn run(&self, inv: Invocation) -> std::io::Result<Finished> {
                Ok(match inv.args.first().map(String::as_str) {
                    Some("--version") => {
                        Finished { code: Some(0), stdout: "2.1.0".into(), ..Default::default() }
                    }
                    _ => Finished {
                        code: Some(1),
                        stdout: r#"{"loggedIn":false}"#.into(),
                        ..Default::default()
                    },
                })
            }
        }
        let exe = std::env::current_exe().unwrap();
        let p = LocalAgentProvider::with_runner(
            "p",
            "Mine",
            "",
            LocalAgentKind::ClaudeCode,
            exe.to_str(),
            Arc::new(SignedOut),
        );
        let e = p.probe().await.unwrap_err();
        assert!(e.to_string().contains("not signed in"), "{e}");
    }
}
