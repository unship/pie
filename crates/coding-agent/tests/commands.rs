//! Integration test for the slash-command registry. Drives `dispatch` against a real
//! `AgentHarness` (faux stream) and verifies user-visible effects: `/thinking high` flips the
//! harness's thinking level *and* writes a thinking_level_change row to the session, so
//! `--resume` later restores it.

use std::path::Path;
use std::sync::Arc;

use pie_agent_core::{
    AgentHarness, AgentHarnessOptions, MemorySessionStorage, Session, SessionStorage,
    SessionTreeEntry, ThinkingLevel,
};

// The binary crate doesn't expose `commands` — pull it in via path-include so this test
// exercises the actual code path without restructuring the crate as a [lib]. `commands.rs`
// references `crate::export`, so we include those siblings too. They appear unused-from-tests
// (no items are called directly here) — that's fine; the commands module reaches into them.
#[path = "../src/commands.rs"]
mod commands;
#[allow(dead_code)]
#[path = "../src/config.rs"]
mod config;
#[allow(dead_code)]
#[path = "../src/export.rs"]
mod export;

fn faux_model() -> pie_ai::Model {
    pie_ai::Model {
        id: "faux".into(),
        name: "Faux".into(),
        api: pie_ai::Api::from("faux"),
        provider: pie_ai::Provider::from("faux"),
        base_url: String::new(),
        reasoning: false,
        thinking_level_map: None,
        input: vec![],
        cost: pie_ai::ModelCost::default(),
        context_window: 0,
        max_tokens: 0,
        headers: None,
        compat: None,
    }
}

#[tokio::test]
async fn dispatch_thinking_command_updates_state_and_session() {
    let storage = Arc::new(MemorySessionStorage::new());
    let session = Session::new(storage as Arc<dyn SessionStorage>);
    let mut opts = AgentHarnessOptions::new(faux_model(), session.clone());
    opts.thinking_level = ThinkingLevel::Off;
    let harness = Arc::new(AgentHarness::new(opts));

    let registry = commands::Registry::with_builtins();
    let cwd = std::env::current_dir().unwrap();
    let ctx = commands::CommandCtx {
        harness: &harness,
        session_id: "test",
        log_path: None,
        tool_count: 0,
        cwd: &cwd,
    };

    let outcome = commands::dispatch("/thinking high", &registry, &ctx).await;
    assert!(matches!(outcome, commands::CommandOutcome::Handled));

    assert_eq!(
        harness.agent().state().thinking_level,
        Some(ThinkingLevel::High)
    );
    let entries = session.entries().await.unwrap();
    let saw_change = entries.iter().any(|e| {
        matches!(
            e,
            SessionTreeEntry::ThinkingLevelChange { thinking_level, .. } if thinking_level == "high"
        )
    });
    assert!(
        saw_change,
        "thinking_level_change entry must be persisted: {entries:#?}"
    );
}

#[tokio::test]
async fn dispatch_unknown_command_returns_error_outcome() {
    let storage = Arc::new(MemorySessionStorage::new());
    let session = Session::new(storage as Arc<dyn SessionStorage>);
    let opts = AgentHarnessOptions::new(faux_model(), session);
    let harness = Arc::new(AgentHarness::new(opts));

    let registry = commands::Registry::with_builtins();
    let cwd = std::env::current_dir().unwrap();
    let ctx = commands::CommandCtx {
        harness: &harness,
        session_id: "test",
        log_path: None,
        tool_count: 0,
        cwd: &cwd,
    };
    let outcome = commands::dispatch("/notarealcommand", &registry, &ctx).await;
    match outcome {
        commands::CommandOutcome::Error(msg) => assert!(msg.contains("unknown command")),
        other => panic!("expected Error outcome, got {other:?}"),
    }
}

#[tokio::test]
async fn dispatch_undo_removes_last_turn_from_active_branch() {
    use pie_agent_core::StreamFn;
    use pie_ai::{
        AssistantMessage, AssistantMessageEvent, AssistantMessageEventStream, AssistantRole,
        ContentBlock, DoneReason, StopReason, Usage,
    };

    fn faux_stream(text: &'static str) -> StreamFn {
        Arc::new(move |_, _, _| {
            let (stream, mut sender) = AssistantMessageEventStream::new();
            tokio::spawn(async move {
                let msg = AssistantMessage {
                    role: AssistantRole::Assistant,
                    content: vec![ContentBlock::text(text)],
                    api: pie_ai::Api::from("faux"),
                    provider: pie_ai::Provider::from("faux"),
                    model: "faux".into(),
                    response_model: None,
                    response_id: None,
                    diagnostics: None,
                    usage: Usage::default(),
                    stop_reason: StopReason::Stop,
                    error_message: None,
                    timestamp: 0,
                };
                sender.push(AssistantMessageEvent::Start {
                    partial: msg.clone(),
                });
                sender.push(AssistantMessageEvent::Done {
                    reason: DoneReason::Stop,
                    message: msg,
                });
            });
            stream
        })
    }

    let storage = Arc::new(MemorySessionStorage::new());
    let session = Session::new(storage as Arc<dyn SessionStorage>);
    let mut opts = AgentHarnessOptions::new(faux_model(), session.clone());
    opts.stream_fn = Some(faux_stream("ack-1"));
    let harness = Arc::new(AgentHarness::new(opts));
    harness.prompt("hi").await.unwrap();

    // Sanity: there are now 2 messages on the active branch (1 user, 1 assistant).
    let before = session.build_context().await.unwrap().messages.len();
    assert_eq!(before, 2);

    let registry = commands::Registry::with_builtins();
    let cwd = std::env::current_dir().unwrap();
    let ctx = commands::CommandCtx {
        harness: &harness,
        session_id: "test",
        log_path: None,
        tool_count: 0,
        cwd: &cwd,
    };
    let outcome = commands::dispatch("/undo", &registry, &ctx).await;
    assert!(matches!(outcome, commands::CommandOutcome::Handled));

    let after = session.build_context().await.unwrap().messages.len();
    assert_eq!(
        after, 0,
        "after /undo, both user + assistant should be off the active branch"
    );
}

#[tokio::test]
async fn dispatch_quit_returns_quit_outcome() {
    let storage = Arc::new(MemorySessionStorage::new());
    let session = Session::new(storage as Arc<dyn SessionStorage>);
    let opts = AgentHarnessOptions::new(faux_model(), session);
    let harness = Arc::new(AgentHarness::new(opts));

    let registry = commands::Registry::with_builtins();
    let cwd = std::env::current_dir().unwrap();
    let ctx = commands::CommandCtx {
        harness: &harness,
        session_id: "test",
        log_path: None,
        tool_count: 0,
        cwd: &cwd,
    };

    for input in ["/quit", "/exit", "/q"] {
        let outcome = commands::dispatch(input, &registry, &ctx).await;
        assert!(
            matches!(outcome, commands::CommandOutcome::Quit),
            "{input} should map to Quit"
        );
    }
}

// The path-include duplicates the module, so we silence the dead-code warning about helpers
// that only the binary calls.
#[allow(dead_code)]
fn _path_check(_p: &Path) {}
