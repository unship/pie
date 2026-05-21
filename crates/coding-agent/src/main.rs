//! pie-coding-agent — minimal coding agent CLI on top of pie-agent-core.
//!
//! Modeled on `packages/coding-agent/` (the TS implementation) in spirit: same tools
//! (`read`/`write`/`edit`/`bash`/`ls`/`grep`/`find` + `memory`), same `--resume` semantics
//! scoped by cwd hash, same "interactive TUI" mode, dual-root skills loader (project ↻ user).
//! Trimmed scope: no extensions, no themes, no print/rpc/json modes.

mod agent_session;
mod config;
mod model;
mod session;
mod skills;
mod tools;
mod tui;

use std::io::Write as _;
use std::time::{Duration, Instant};

use tokio::io::AsyncBufReadExt as _;

use anyhow::{Context, Result};
use clap::Parser;
use pie_agent_core::{
    AgentHarness, AgentHarnessOptions, AgentMessage, JsonlSessionRepo, SessionContext,
    ThinkingLevel,
};
use pie_ai::Message as PiMessage;

#[derive(Parser, Debug)]
#[command(
    name = "pie",
    version,
    about = "Simple coding agent on top of pie-agent-core"
)]
struct Cli {
    /// Provider id (anthropic, openai, openrouter, …). When unset, auto-detected from env.
    #[arg(long)]
    provider: Option<String>,
    /// Model id within the provider's catalog.
    #[arg(long)]
    model: Option<String>,
    /// Thinking level (off | minimal | low | medium | high | xhigh).
    #[arg(long, default_value = "off")]
    thinking: String,

    /// Resume the most recent session for this cwd (or pass --resume-id for a specific one).
    #[arg(long)]
    resume: bool,
    /// Resume a specific session by id (full UUIDv7 or a unique prefix).
    #[arg(long, value_name = "ID")]
    resume_id: Option<String>,

    /// List sessions for this cwd and exit.
    #[arg(long)]
    list_sessions: bool,
    /// Delete a session by id and exit.
    #[arg(long, value_name = "ID")]
    delete_session: Option<String>,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let cwd = std::env::current_dir().context("getting cwd")?;
    let repo = session::open_repo(&cwd).await;

    if cli.list_sessions {
        return list_sessions_cmd(&repo).await;
    }
    if let Some(id) = &cli.delete_session {
        return delete_session_cmd(&repo, id).await;
    }

    run_repl(cli, cwd, repo).await
}

async fn list_sessions_cmd(repo: &JsonlSessionRepo) -> Result<()> {
    let entries = session::list_entries(repo).await?;
    if entries.is_empty() {
        println!("(no sessions for this cwd)");
        return Ok(());
    }
    println!("sessions in {}:", repo.root().display());
    for e in entries {
        let preview = e.preview.as_deref().unwrap_or("");
        println!(
            "  {}  {}  {}",
            &e.id[..16.min(e.id.len())],
            e.created_at,
            preview
        );
    }
    Ok(())
}

async fn delete_session_cmd(repo: &JsonlSessionRepo, id: &str) -> Result<()> {
    let path = session::delete_by_id(repo, id).await?;
    println!("deleted {}", path.display());
    Ok(())
}

async fn run_repl(cli: Cli, cwd: std::path::PathBuf, repo: JsonlSessionRepo) -> Result<()> {
    let model = model::auto_detect_model(cli.provider.as_deref(), cli.model.as_deref())?;
    let thinking = parse_thinking(&cli.thinking)?;

    // Resolve / create the session.
    let (session, resumed) = if cli.resume || cli.resume_id.is_some() {
        let s = session::resume(&repo, cli.resume_id.as_deref()).await?;
        (s, true)
    } else {
        let s = session::create(&repo, &cwd).await?;
        (s, false)
    };
    let session_id = session
        .storage()
        .get_metadata_json()
        .await?
        .get("id")
        .and_then(|v| v.as_str())
        .unwrap_or("?")
        .to_string();

    // Build the harness.
    let memory_dir = config::memory_dir();
    let tools = tools::default_tools(memory_dir.clone());
    let tool_names = tools
        .iter()
        .map(|tool| tool.definition().name.clone())
        .collect::<Vec<_>>();
    let memory_block = tools::memory::load_memory_block(&memory_dir).await;
    let system_prompt = compose_system_prompt(&cwd, &memory_block, &tool_names);

    let loaded_skills = skills::load_all(&cwd).await;

    let mut opts = AgentHarnessOptions::new(model.clone(), session.clone());
    opts.system_prompt = system_prompt;
    opts.thinking_level = thinking;
    opts.tools = tools;
    opts.skills = loaded_skills.skills.clone();
    let harness = std::sync::Arc::new(AgentHarness::new(opts));
    let session_runner =
        agent_session::AgentSession::new(harness.clone(), agent_session::RetrySettings::default());

    // Banner + replay (if --resume). All resume hydration lives on AgentHarness, so the CLI
    // just asks for the rebuilt SessionContext and renders it.
    let tui = tui::Tui::new();
    let replay_context = if resumed {
        Some(harness.rehydrate_from_session().await?)
    } else {
        None
    };
    let display_model = harness
        .agent()
        .state()
        .model
        .clone()
        .unwrap_or_else(|| model.clone());
    tui.banner(&display_model, &session_id, resumed, &tool_names);
    if !loaded_skills.skills.is_empty() {
        tui.system_line(&format!(
            "loaded {} skill(s): {}",
            loaded_skills.skills.len(),
            loaded_skills
                .skills
                .iter()
                .map(|s| s.name.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    if !loaded_skills.diagnostics.is_empty() {
        tui.system_line(&format!(
            "skills loader: {} diagnostic(s), first: {}",
            loaded_skills.diagnostics.len(),
            loaded_skills.diagnostics[0].message
        ));
    }
    if let Some(ctx) = replay_context.as_ref() {
        replay_transcript(ctx, &tui);
    }

    // Wire the TUI listener so each prompt's events stream live.
    let _unsub = harness.agent().subscribe(tui.listener());

    // REPL — async stdin so we can race a Ctrl-C abort against the in-flight prompt.
    let mut stdin = tokio::io::BufReader::new(tokio::io::stdin()).lines();
    let mut last_idle_ctrlc: Option<Instant> = None;

    loop {
        tui.user_prompt_marker();

        // Idle read with double-Ctrl-C-to-exit semantics. tokio's ctrl_c() yields each time a
        // SIGINT arrives — so awaiting it once gets the next signal cleanly.
        let line = tokio::select! {
            line = stdin.next_line() => match line {
                Ok(Some(l)) => l,
                Ok(None) => { tui.system_line("eof — exiting"); break; }
                Err(e) => { tui.error_line(&format!("{e}")); break; }
            },
            _ = tokio::signal::ctrl_c() => {
                let now = Instant::now();
                if last_idle_ctrlc
                    .map(|t| now.duration_since(t) < Duration::from_millis(1500))
                    .unwrap_or(false)
                {
                    tui.system_line("bye");
                    break;
                }
                last_idle_ctrlc = Some(now);
                tui.system_line("press Ctrl-C again within 1.5s to exit, or type /quit");
                continue;
            }
        };

        // Successful input clears the "second-Ctrl-C-to-exit" arming.
        last_idle_ctrlc = None;

        let input = line.trim();
        if input.is_empty() {
            continue;
        }
        if input == "/quit" || input == "/exit" || input == "/q" {
            tui.system_line("bye");
            break;
        }
        if input == "/help" {
            print_help();
            continue;
        }
        if input == "/clear" {
            // Clear screen via ANSI; conversation state is unchanged.
            print!("\x1b[2J\x1b[H");
            let _ = std::io::stdout().flush();
            continue;
        }
        if input == "/skills" {
            print_skills(&harness);
            continue;
        }

        // Run the prompt while watching for Ctrl-C. On signal, ask the harness to abort and
        // keep awaiting the future so it cleans up; the result tells us whether it aborted.
        let aborted = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let aborted_for_signal = aborted.clone();
        let harness_for_signal = harness.clone();
        let prompt_fut = session_runner.prompt(input.to_string());
        tokio::pin!(prompt_fut);
        let signal_fut = async move {
            // First Ctrl-C while a prompt is in flight: abort.
            let _ = tokio::signal::ctrl_c().await;
            harness_for_signal.abort();
            aborted_for_signal.store(true, std::sync::atomic::Ordering::SeqCst);
        };
        tokio::pin!(signal_fut);

        // `biased` keeps the prompt future polled first so a fast completion doesn't get
        // pre-empted by a stale signal future.
        let res = loop {
            tokio::select! {
                biased;
                res = &mut prompt_fut => break res,
                _ = &mut signal_fut, if !aborted.load(std::sync::atomic::Ordering::SeqCst) => {
                    // The signal future will not re-arm after firing; subsequent ctrl_c during
                    // the same turn falls through to default tokio handling. Loop back to
                    // continue awaiting the prompt future for clean unwind.
                }
            }
        };

        if aborted.load(std::sync::atomic::Ordering::SeqCst) {
            tui.system_line("[aborted]");
        } else if let Err(e) = res {
            tui.error_line(&format!("{e}"));
        }
    }
    Ok(())
}

fn print_help() {
    println!();
    println!("Commands:");
    println!("  /help          show this help");
    println!("  /skills        list loaded skills");
    println!("  /clear         clear screen (keeps history)");
    println!("  /quit | /q     exit");
    println!();
    println!("Anything else is sent as a prompt to the agent.");
    println!();
}

fn print_skills(harness: &AgentHarness) {
    let skills = harness.skills();
    if skills.is_empty() {
        println!(
            "(no skills loaded — drop SKILL.md files under ~/.pie/skills/<name>/ or <cwd>/.pie/skills/<name>/)"
        );
        return;
    }
    println!("Loaded skills ({}):", skills.len());
    for s in &skills {
        println!("  - {}  ({})", s.name, s.file_path);
        if !s.description.is_empty() {
            println!("      {}", s.description);
        }
    }
}

fn parse_thinking(s: &str) -> Result<ThinkingLevel> {
    s.parse().map_err(anyhow::Error::msg)
}

fn compose_system_prompt(cwd: &std::path::Path, memory: &str, tool_names: &[String]) -> String {
    let mut s = String::new();
    s.push_str(&render_base_prompt(tool_names));
    s.push_str("\n\n");
    s.push_str(&format!("Current working directory: {}\n", cwd.display()));
    if !memory.is_empty() {
        s.push('\n');
        s.push_str(memory);
        s.push('\n');
    }
    s
}

/// Build the prompt header. The tool inventory is rendered from the actual registered tool
/// definitions so adding/removing a tool in `tools::default_tools()` flows through here without
/// a hand-edited literal list.
fn render_base_prompt(tool_names: &[String]) -> String {
    let inventory = if tool_names.is_empty() {
        "no tools registered".to_string()
    } else {
        tool_names.join(", ")
    };
    format!(
        "You are pie-coding-agent, a minimal coding assistant running in a terminal. \
You have access to the following tools: {inventory}. \
Prefer running a tool over guessing. When making file changes, read the file first to confirm the exact current contents, then edit or write. Keep responses concise."
    )
}

fn replay_transcript(ctx: &SessionContext, tui: &tui::Tui) {
    if ctx.messages.is_empty() {
        return;
    }
    tui.system_line(&format!(
        "resumed — replaying {} messages",
        ctx.messages.len()
    ));
    for m in &ctx.messages {
        tui::render_persisted(m);
    }
    // Skip custom variants (compaction_summary etc.); they aren't model-visible here. But the
    // harness uses them via convert_to_llm filtering — that's already handled by pie-agent-core.
    drop_unused(&ctx.messages);
}

fn drop_unused(_: &[AgentMessage]) {}

/// Helper for callers that want to feed a Message (raw pie-ai role variant) into the agent. Not
/// directly used by the REPL but kept here for the tests.
pub fn user_message(text: &str) -> AgentMessage {
    AgentMessage::Llm(PiMessage::User(pie_ai::UserMessage {
        role: pie_ai::UserRole::User,
        content: pie_ai::UserContent::Text(text.into()),
        timestamp: chrono::Utc::now().timestamp_millis(),
    }))
}
