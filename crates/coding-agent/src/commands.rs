//! Slash-command registry. Tracks a small set of REPL builtins and dispatches by name.
//!
//! Built-in commands today: `/help`, `/clear`, `/skills`, `/quit` (and aliases), `/model`,
//! `/thinking`. The trait is shaped so future extensions (issue #10 Part B) can register
//! additional commands without touching this file.

use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use pie_agent_core::{AgentHarness, ThinkingLevel};
use pie_ai::{Provider, get_model};

/// Outcome of running a command. Drives the REPL's next action.
#[derive(Debug)]
pub enum CommandOutcome {
    /// Continue the REPL loop normally.
    Handled,
    /// Quit the REPL cleanly.
    Quit,
    /// Clear the screen — REPL handles the ANSI escape so we don't bake it into commands.
    ClearScreen,
    /// Command surfaced an error message; REPL renders it via `tui.error_line`.
    Error(String),
}

/// Context handed to a command at runtime. Kept narrow so each command's dependencies are
/// explicit.
pub struct CommandCtx<'a> {
    pub harness: &'a Arc<AgentHarness>,
    pub session_id: &'a str,
    pub log_path: Option<&'a PathBuf>,
    pub tool_count: usize,
    pub cwd: &'a std::path::Path,
}

#[async_trait]
pub trait SlashCommand: Send + Sync {
    /// Canonical name without the leading `/`.
    fn name(&self) -> &'static str;
    /// Optional aliases (also without leading `/`).
    fn aliases(&self) -> &'static [&'static str] {
        &[]
    }
    fn description(&self) -> &'static str;
    /// Optional argument hint shown in `/help`. Empty when the command takes no arguments.
    fn usage(&self) -> &'static str {
        ""
    }
    async fn run(&self, argv: &[String], ctx: &CommandCtx<'_>) -> CommandOutcome;
}

/// In-memory registry. Lookups are linear scans over a small set — `O(n)` is fine.
pub struct Registry {
    commands: Vec<Arc<dyn SlashCommand>>,
}

impl Registry {
    pub fn new() -> Self {
        Self {
            commands: Vec::new(),
        }
    }

    pub fn with_builtins() -> Self {
        let mut r = Self::new();
        r.register(Arc::new(HelpCommand));
        r.register(Arc::new(ClearCommand));
        r.register(Arc::new(SkillsCommand));
        r.register(Arc::new(QuitCommand));
        r.register(Arc::new(ModelCommand));
        r.register(Arc::new(ThinkingCommand));
        r.register(Arc::new(CostCommand));
        r.register(Arc::new(DiagCommand));
        r.register(Arc::new(TemplateCommand));
        r.register(Arc::new(SaveCommand));
        r.register(Arc::new(CompactCommand));
        r.register(Arc::new(UndoCommand));
        r
    }

    pub fn register(&mut self, command: Arc<dyn SlashCommand>) {
        self.commands.push(command);
    }

    pub fn commands(&self) -> &[Arc<dyn SlashCommand>] {
        &self.commands
    }

    /// Lookup by name or alias. `name` is the bare command without `/`.
    pub fn find(&self, name: &str) -> Option<Arc<dyn SlashCommand>> {
        self.commands
            .iter()
            .find(|c| c.name() == name || c.aliases().contains(&name))
            .cloned()
    }
}

impl Default for Registry {
    fn default() -> Self {
        Self::with_builtins()
    }
}

/// Split `/cmd arg1 "arg with spaces"` into `(cmd, [arg1, arg with spaces])`. Returns `None`
/// if `input` doesn't start with `/`. Quoting is minimal: balanced double quotes only.
pub fn parse(input: &str) -> Option<(String, Vec<String>)> {
    let trimmed = input.trim_start();
    let body = trimmed.strip_prefix('/')?;
    let mut argv: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut in_quotes = false;
    for c in body.chars() {
        match c {
            '"' => in_quotes = !in_quotes,
            ' ' | '\t' if !in_quotes => {
                if !current.is_empty() {
                    argv.push(std::mem::take(&mut current));
                }
            }
            other => current.push(other),
        }
    }
    if !current.is_empty() {
        argv.push(current);
    }
    if argv.is_empty() {
        // Bare `/` — no command name.
        return None;
    }
    let name = argv.remove(0);
    Some((name, argv))
}

// ──────────────────────────────────────────────────────────────────────────────────────────
// Builtins
// ──────────────────────────────────────────────────────────────────────────────────────────

struct HelpCommand;

#[async_trait]
impl SlashCommand for HelpCommand {
    fn name(&self) -> &'static str {
        "help"
    }
    fn description(&self) -> &'static str {
        "show available commands"
    }
    async fn run(&self, _argv: &[String], _ctx: &CommandCtx<'_>) -> CommandOutcome {
        // The REPL's `print_help` walks the registry — see main.rs. This handler is a stub
        // because Help needs the Registry itself, which we don't pass into commands. The
        // REPL detects `/help` before dispatch.
        CommandOutcome::Handled
    }
}

struct ClearCommand;

#[async_trait]
impl SlashCommand for ClearCommand {
    fn name(&self) -> &'static str {
        "clear"
    }
    fn description(&self) -> &'static str {
        "clear screen (keeps conversation history)"
    }
    async fn run(&self, _argv: &[String], _ctx: &CommandCtx<'_>) -> CommandOutcome {
        CommandOutcome::ClearScreen
    }
}

struct SkillsCommand;

#[async_trait]
impl SlashCommand for SkillsCommand {
    fn name(&self) -> &'static str {
        "skills"
    }
    fn description(&self) -> &'static str {
        "list loaded skills"
    }
    async fn run(&self, _argv: &[String], ctx: &CommandCtx<'_>) -> CommandOutcome {
        let skills = ctx.harness.skills();
        if skills.is_empty() {
            println!(
                "(no skills loaded — drop SKILL.md files under ~/.pie/skills/<name>/ or <cwd>/.pie/skills/<name>/)"
            );
        } else {
            println!("Loaded skills ({}):", skills.len());
            for s in &skills {
                println!("  - {}  ({})", s.name, s.file_path);
                if !s.description.is_empty() {
                    println!("      {}", s.description);
                }
            }
        }
        CommandOutcome::Handled
    }
}

struct QuitCommand;

#[async_trait]
impl SlashCommand for QuitCommand {
    fn name(&self) -> &'static str {
        "quit"
    }
    fn aliases(&self) -> &'static [&'static str] {
        &["exit", "q"]
    }
    fn description(&self) -> &'static str {
        "exit the REPL"
    }
    async fn run(&self, _argv: &[String], _ctx: &CommandCtx<'_>) -> CommandOutcome {
        CommandOutcome::Quit
    }
}

struct ModelCommand;

#[async_trait]
impl SlashCommand for ModelCommand {
    fn name(&self) -> &'static str {
        "model"
    }
    fn description(&self) -> &'static str {
        "show or switch the active model"
    }
    fn usage(&self) -> &'static str {
        "[provider:model-id]"
    }
    async fn run(&self, argv: &[String], ctx: &CommandCtx<'_>) -> CommandOutcome {
        if argv.is_empty() {
            let current = ctx.harness.agent().state().model.clone();
            match current {
                Some(m) => println!("active model: {}:{}", m.provider.0, m.id),
                None => println!("(no model active)"),
            }
            return CommandOutcome::Handled;
        }
        // Accept either `provider:id` or two separate args `provider id`.
        let spec = argv.join(" ");
        let (provider, id) = match spec.split_once(':') {
            Some((p, i)) => (p.to_string(), i.to_string()),
            None => {
                return CommandOutcome::Error(
                    "expected provider:model-id, e.g. /model anthropic:claude-haiku-4-5".into(),
                );
            }
        };
        let provider_obj = Provider::from(provider.as_str());
        let Some(model) = get_model(&provider_obj, &id) else {
            return CommandOutcome::Error(format!("unknown model in catalog: {provider}:{id}"));
        };
        match ctx.harness.set_model(model.clone()).await {
            Ok(_) => {
                println!("switched to {provider}:{id}");
                CommandOutcome::Handled
            }
            Err(e) => CommandOutcome::Error(format!("set_model failed: {e}")),
        }
    }
}

struct ThinkingCommand;

#[async_trait]
impl SlashCommand for ThinkingCommand {
    fn name(&self) -> &'static str {
        "thinking"
    }
    fn description(&self) -> &'static str {
        "show or set the thinking level"
    }
    fn usage(&self) -> &'static str {
        "[off|minimal|low|medium|high|xhigh]"
    }
    async fn run(&self, argv: &[String], ctx: &CommandCtx<'_>) -> CommandOutcome {
        if argv.is_empty() {
            let lvl = ctx.harness.agent().state().thinking_level;
            println!("thinking level: {}", lvl.map(|l| l.as_str()).unwrap_or("?"));
            return CommandOutcome::Handled;
        }
        let raw = argv[0].to_lowercase();
        let level: ThinkingLevel = match raw.parse() {
            Ok(l) => l,
            Err(e) => {
                return CommandOutcome::Error(format!("invalid level: {e}"));
            }
        };
        match ctx.harness.set_thinking_level(level).await {
            Ok(_) => {
                println!("thinking level: {}", level.as_str());
                CommandOutcome::Handled
            }
            Err(e) => CommandOutcome::Error(format!("set_thinking_level failed: {e}")),
        }
    }
}

// Re-export for `print_help` in main.rs.
pub fn print_help(registry: &Registry) {
    println!();
    println!("Commands:");
    for cmd in registry.commands() {
        let aliases = if cmd.aliases().is_empty() {
            String::new()
        } else {
            format!(" (aliases: {})", cmd.aliases().join(", "))
        };
        let usage = if cmd.usage().is_empty() {
            String::new()
        } else {
            format!(" {}", cmd.usage())
        };
        println!(
            "  /{}{}    {}{}",
            cmd.name(),
            usage,
            cmd.description(),
            aliases
        );
    }
    println!();
    println!("Anything else is sent as a prompt to the agent.");
    println!();
}

struct CostCommand;

#[async_trait]
impl SlashCommand for CostCommand {
    fn name(&self) -> &'static str {
        "cost"
    }
    fn description(&self) -> &'static str {
        "show running token / USD totals for this session"
    }
    fn usage(&self) -> &'static str {
        "[reset]"
    }
    async fn run(&self, argv: &[String], ctx: &CommandCtx<'_>) -> CommandOutcome {
        if argv.first().map(|s| s.as_str()) == Some("reset") {
            ctx.harness.reset_cost();
            println!("cost counters reset");
            return CommandOutcome::Handled;
        }
        let snap = ctx.harness.cost();
        println!("{}", pie_agent_core::cost_full_breakdown(&snap));
        CommandOutcome::Handled
    }
}

struct DiagCommand;

#[async_trait]
impl SlashCommand for DiagCommand {
    fn name(&self) -> &'static str {
        "diag"
    }
    fn description(&self) -> &'static str {
        "show diagnostic info (model, thinking, cost, log path)"
    }
    async fn run(&self, _argv: &[String], ctx: &CommandCtx<'_>) -> CommandOutcome {
        let state = ctx.harness.agent().state();
        let model = state
            .model
            .as_ref()
            .map(|m| format!("{}:{}", m.provider.0, m.id))
            .unwrap_or_else(|| "(none)".into());
        let thinking = state
            .thinking_level
            .map(|l| l.as_str())
            .unwrap_or("?")
            .to_string();
        let skill_count = ctx.harness.skills().len();
        let cost = ctx.harness.cost();
        let log = ctx
            .log_path
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "(logging disabled)".into());
        println!();
        println!("Diagnostic snapshot:");
        println!("  session       {}", ctx.session_id);
        println!("  model         {model}");
        println!("  thinking      {thinking}");
        println!("  tools         {}", ctx.tool_count);
        println!("  skills        {skill_count}");
        println!(
            "  cost          {}",
            pie_agent_core::cost_one_line_summary(&cost)
        );
        println!("  log file      {log}");
        println!();
        CommandOutcome::Handled
    }
}

struct TemplateCommand;

#[async_trait]
impl SlashCommand for TemplateCommand {
    fn name(&self) -> &'static str {
        "template"
    }
    fn description(&self) -> &'static str {
        "list templates, or run one with /template <name> [k=v ...]"
    }
    fn usage(&self) -> &'static str {
        "[name] [k=v ...]"
    }
    async fn run(&self, argv: &[String], ctx: &CommandCtx<'_>) -> CommandOutcome {
        if argv.is_empty() {
            let templates = ctx.harness.templates();
            if templates.is_empty() {
                println!(
                    "(no templates loaded — drop `.md` files under ~/.pie/templates/ or <cwd>/.pie/templates/)"
                );
            } else {
                println!("Loaded templates ({}):", templates.len());
                for t in &templates {
                    let desc = t.description.clone().unwrap_or_default();
                    println!("  /template {}  {}", t.name, desc);
                }
            }
            return CommandOutcome::Handled;
        }
        let name = argv[0].clone();
        // Remaining args are `k=v` pairs.
        let mut vars = serde_json::Map::new();
        for arg in &argv[1..] {
            if let Some((k, v)) = arg.split_once('=') {
                vars.insert(k.to_string(), serde_json::Value::String(v.to_string()));
            } else {
                return CommandOutcome::Error(format!("expected k=v argument; got: {arg}"));
            }
        }
        match ctx.harness.prompt_from_template(&name, vars).await {
            Ok(()) => CommandOutcome::Handled,
            Err(e) => CommandOutcome::Error(format!("template run failed: {e}")),
        }
    }
}

struct SaveCommand;

#[async_trait]
impl SlashCommand for SaveCommand {
    fn name(&self) -> &'static str {
        "save"
    }
    fn description(&self) -> &'static str {
        "export session transcript to Markdown"
    }
    fn usage(&self) -> &'static str {
        "[path]"
    }
    async fn run(&self, argv: &[String], ctx: &CommandCtx<'_>) -> CommandOutcome {
        let dest = if let Some(path) = argv.first() {
            std::path::PathBuf::from(path)
        } else {
            crate::export::default_export_path(ctx.session_id)
        };
        // If the path is relative, resolve against cwd so /save foo.md lands where the user
        // expects (and not in some random working dir).
        let dest = if dest.is_absolute() {
            dest
        } else {
            ctx.cwd.join(dest)
        };
        match crate::export::save(ctx.harness.session(), &dest).await {
            Ok(p) => {
                println!("saved transcript: {}", p.display());
                CommandOutcome::Handled
            }
            Err(e) => CommandOutcome::Error(format!("save failed: {e}")),
        }
    }
}

struct CompactCommand;

#[async_trait]
impl SlashCommand for CompactCommand {
    fn name(&self) -> &'static str {
        "compact"
    }
    fn description(&self) -> &'static str {
        "force a context compaction now (no-op when nothing to summarize)"
    }
    fn usage(&self) -> &'static str {
        "[\"custom instructions\"]"
    }
    async fn run(&self, argv: &[String], ctx: &CommandCtx<'_>) -> CommandOutcome {
        let custom = if argv.is_empty() {
            None
        } else {
            Some(argv.join(" "))
        };
        match ctx.harness.force_compact(custom).await {
            Ok(true) => {
                println!("compaction ran");
                CommandOutcome::Handled
            }
            Ok(false) => {
                println!("nothing to compact");
                CommandOutcome::Handled
            }
            Err(e) => CommandOutcome::Error(format!("compaction failed: {e}")),
        }
    }
}

struct UndoCommand;

#[async_trait]
impl SlashCommand for UndoCommand {
    fn name(&self) -> &'static str {
        "undo"
    }
    fn description(&self) -> &'static str {
        "remove the most recent user+assistant turn from the active branch"
    }
    async fn run(&self, _argv: &[String], ctx: &CommandCtx<'_>) -> CommandOutcome {
        let session = ctx.harness.session();
        let path = match session.branch(None).await {
            Ok(p) => p,
            Err(e) => return CommandOutcome::Error(format!("read branch: {e}")),
        };
        // Walk backwards for the most recent Message that's a User. That message is the
        // start of the turn we want to drop.
        let mut target_parent: Option<String> = None;
        let mut found = false;
        for entry in path.iter().rev() {
            if let pie_agent_core::SessionTreeEntry::Message {
                message: pie_agent_core::AgentMessage::Llm(pie_ai::Message::User(_)),
                parent_id,
                ..
            } = entry
            {
                target_parent = parent_id.clone();
                found = true;
                break;
            }
        }
        if !found {
            return CommandOutcome::Error("no user message to undo".into());
        }
        match ctx.harness.move_to(target_parent.as_deref(), None).await {
            Ok(_) => {
                println!("undid last turn");
                CommandOutcome::Handled
            }
            Err(e) => CommandOutcome::Error(format!("undo failed: {e}")),
        }
    }
}

pub async fn dispatch(input: &str, registry: &Registry, ctx: &CommandCtx<'_>) -> CommandOutcome {
    let (name, argv) = match parse(input) {
        Some(parts) => parts,
        None => return CommandOutcome::Error("not a slash command".into()),
    };
    // Special-case `/help`: the handler can't see the registry, so we render here.
    if name == "help" {
        print_help(registry);
        return CommandOutcome::Handled;
    }
    let Some(cmd) = registry.find(&name) else {
        return CommandOutcome::Error(format!("unknown command: /{name} (try /help)"));
    };
    cmd.run(&argv, ctx).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_splits_on_whitespace() {
        let (name, args) = parse("/model anthropic:claude").unwrap();
        assert_eq!(name, "model");
        assert_eq!(args, vec!["anthropic:claude".to_string()]);
    }

    #[test]
    fn parse_keeps_quoted_args_together() {
        let (name, args) = parse("/say \"hello world\" again").unwrap();
        assert_eq!(name, "say");
        assert_eq!(args, vec!["hello world".to_string(), "again".to_string()]);
    }

    #[test]
    fn parse_returns_none_for_non_slash() {
        assert!(parse("hello world").is_none());
        assert!(parse("/").is_none());
    }

    #[test]
    fn registry_lookup_by_name_and_alias() {
        let r = Registry::with_builtins();
        assert!(r.find("quit").is_some());
        assert!(r.find("q").is_some());
        assert!(r.find("exit").is_some());
        assert!(r.find("nope").is_none());
    }
}
