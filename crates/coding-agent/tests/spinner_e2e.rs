//! End-to-end test for the spinner's integration with the agent's event stream.
//!
//! Drives a real AgentHarness + a faux StreamFn whose first text delta arrives only after
//! a deliberate delay. Subscribes the same kind of listener main.rs installs (calls
//! `stop_sync` on TextDelta / ThinkingDelta / tool execution). Captures the spinner's
//! stderr-equivalent via the BufferSink test hook. Asserts:
//!
//! - Frame 0 is in the captured buffer BEFORE the LLM event arrives (synchronous render).
//! - Multiple frames render during the delay (animation actually runs).
//! - On first text delta the `\r\x1b[2K` clear is appended.
//! - After stop, no further frames are written (animation task exits).

use std::sync::Arc;
use std::time::Duration;

use pie_agent_core::{
    AgentEvent, AgentHarness, AgentHarnessOptions, MemorySessionStorage, Session, SessionStorage,
    StreamFn,
};
use pie_ai::{
    AssistantMessage, AssistantMessageEvent, AssistantMessageEventStream, AssistantRole,
    ContentBlock, DoneReason, ModelCost, StopReason, Usage,
};

#[allow(dead_code)]
#[path = "../src/spinner.rs"]
mod spinner;

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
        cost: ModelCost::default(),
        context_window: 0,
        max_tokens: 0,
        headers: None,
        compat: None,
    }
}

/// A faux stream that sleeps for `delay_ms` before emitting its single text-delta + done.
/// Lets the spinner animate for the delay window before being stopped.
fn delayed_stream(text: &'static str, delay_ms: u64) -> StreamFn {
    Arc::new(move |_, _, _| {
        let (stream, mut sender) = AssistantMessageEventStream::new();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(delay_ms)).await;
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
            // Start (no text yet) → then a single text delta → then done. The spinner
            // listener should fire on the text delta.
            sender.push(AssistantMessageEvent::Start {
                partial: msg.clone(),
            });
            sender.push(AssistantMessageEvent::TextDelta {
                content_index: 0,
                delta: text.to_string(),
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

/// Predicate matching what main.rs installs.
fn should_stop_on(ev: &AgentEvent) -> bool {
    use pie_ai::AssistantMessageEvent;
    match ev {
        AgentEvent::ToolExecutionStart { .. } | AgentEvent::ToolExecutionEnd { .. } => true,
        AgentEvent::MessageUpdate {
            assistant_message_event,
            ..
        } => matches!(
            assistant_message_event,
            AssistantMessageEvent::TextDelta { .. }
                | AssistantMessageEvent::ThinkingDelta { .. }
                | AssistantMessageEvent::ToolCallDelta { .. }
        ),
        _ => false,
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn spinner_shows_during_thinking_then_clears_on_first_delta() {
    let sink = spinner::BufferSink::new();
    let spin = spinner::start_with(
        "thinking",
        Arc::new(sink.clone()) as Arc<dyn spinner::SpinnerSink>,
        true,
    );

    // Frame 0 must already be there before any agent event has fired.
    let initial = sink.as_string();
    assert!(
        initial.contains("⠋") && initial.contains("thinking"),
        "synchronous frame 0 missing: {initial:?}"
    );

    // Wire the agent + listener.
    let storage = Arc::new(MemorySessionStorage::new());
    let session = Session::new(storage as Arc<dyn SessionStorage>);
    let mut opts = AgentHarnessOptions::new(faux_model(), session);
    // 350ms delay before the LLM responds — enough for the spinner to draw a few frames.
    opts.stream_fn = Some(delayed_stream("hello", 350));
    let harness = AgentHarness::new(opts);

    let spin_for_listener = spin.clone();
    let _unsub = harness.agent().subscribe(Arc::new(move |ev, _| {
        let s = spin_for_listener.clone();
        Box::pin(async move {
            if should_stop_on(&ev) {
                s.stop_sync();
            }
        })
    }));

    // Yield once + sleep so the spinner's animation task gets time to render frames
    // before the agent's events arrive. (`tokio::test`'s default current_thread / single-
    // worker runtime can otherwise let the prompt task poll-loop without ever ceding to
    // the spinner's sleep-timer wake.)
    tokio::time::sleep(Duration::from_millis(250)).await;

    // Drive the prompt to completion.
    harness.prompt("hi").await.unwrap();

    // Inspect what was written to the spinner sink:
    let captured = sink.as_string();
    eprintln!("---captured---\n{captured:?}\n---end---");

    // 1. The synchronous clear from stop_sync must be in there.
    assert!(
        captured.contains("\r\x1b[2K"),
        "stop_sync clear escape missing: {captured:?}"
    );

    // 2. At least two distinct frame glyphs (so the animation actually ran for >1 tick).
    let distinct_frames: usize = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"]
        .iter()
        .filter(|f| captured.contains(*f))
        .count();
    assert!(
        distinct_frames >= 2,
        "expected ≥2 distinct frames during the 350ms delay, got {distinct_frames}\n{captured:?}"
    );

    // 3. After the prompt completes, calling stop_sync again is a no-op.
    let before_double_stop = sink.as_string().len();
    spin.stop_sync();
    let after_double_stop = sink.as_string().len();
    assert_eq!(
        before_double_stop, after_double_stop,
        "stop_sync after listener-fired stop should be a no-op"
    );
}

/// Regression: stopping the spinner BEFORE the listener fires (e.g. error path) leaves
/// the buffer in a sane state — frame 0 + a clear — and the animation task exits without
/// adding more frames.
#[tokio::test]
async fn explicit_stop_works_without_any_agent_event() {
    let sink = spinner::BufferSink::new();
    let spin = spinner::start_with(
        "thinking",
        Arc::new(sink.clone()) as Arc<dyn spinner::SpinnerSink>,
        true,
    );

    // Don't wire any listener. Stop immediately.
    spin.stop_sync();

    let s = sink.as_string();
    assert!(s.contains("⠋"), "frame 0: {s:?}");
    assert!(s.ends_with("\r\x1b[2K"), "trailing clear: {s:?}");

    // Wait past frame interval — animation task must not append anything.
    let before = s.len();
    tokio::time::sleep(Duration::from_millis(200)).await;
    let after = sink.as_string().len();
    assert_eq!(after, before, "animation task continued past stop");
}
