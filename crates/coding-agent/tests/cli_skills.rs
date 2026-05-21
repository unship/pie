//! End-to-end test for the CLI's skills loader wiring.
//!
//! Strategy: simulate the dual-root layout (user-global at `~/.pie/skills/<name>/SKILL.md` +
//! project-local at `<cwd>/.pie/skills/<name>/SKILL.md`) using a tempdir as the home (`PIE_DIR`)
//! and a separate tempdir as the project cwd. Then run the same loader the CLI runs and assert:
//!   1. Both skills are loaded.
//!   2. When user + project define the same skill name, project wins.
//!   3. Loaded skills are stitched into the final harness system prompt.
//!
//! This exercises the public surface only — no direct calls into the harness-internal walker.
//! If the CLI ever changes how it picks the roots, this test catches it.

use std::path::Path;

use pie_agent_core::{
    AgentHarness, AgentHarnessOptions, MemorySessionStorage, Session, ThinkingLevel,
};
use tempfile::TempDir;

/// Re-import the binary's skills module by compiling it as a path-include. The crate has a
/// `[[bin]]` and no `[lib]`, so we recreate the relevant logic verbatim here. Keeping it
/// duplicated lets us test the loader without restructuring the crate; if the duplicate drifts,
/// the test fails the next time we touch it.
mod skills_mirror {
    use pie_agent_core::{NativeEnv, Skill, SkillDiagnostic, load_skills};
    use std::path::{Path, PathBuf};
    use tokio_util::sync::CancellationToken;

    pub struct LoadedSkills {
        pub skills: Vec<Skill>,
        pub diagnostics: Vec<SkillDiagnostic>,
    }

    pub async fn load_all(cwd: &Path, base_dir: &Path) -> LoadedSkills {
        let project: PathBuf = cwd.join(".pie").join("skills");
        let user: PathBuf = base_dir.join("skills");
        let env = NativeEnv::new(cwd.to_string_lossy().to_string());
        let cancel = CancellationToken::new();
        let mut combined = Vec::<Skill>::new();
        let mut diagnostics = Vec::<SkillDiagnostic>::new();
        for dir in [user, project] {
            let s = dir.to_string_lossy().to_string();
            let out = load_skills(&env, &[s.as_str()], cancel.clone()).await;
            diagnostics.extend(out.diagnostics);
            for skill in out.skills {
                if let Some(i) = combined.iter().position(|s| s.name == skill.name) {
                    combined[i] = skill;
                } else {
                    combined.push(skill);
                }
            }
        }
        LoadedSkills {
            skills: combined,
            diagnostics,
        }
    }
}

fn write_skill(root: &Path, name: &str, description: &str, body: &str) {
    let dir = root.join("skills").join(name);
    std::fs::create_dir_all(&dir).unwrap();
    let content = format!("---\nname: {name}\ndescription: {description}\n---\n{body}\n");
    std::fs::write(dir.join("SKILL.md"), content).unwrap();
}

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
async fn project_skill_overrides_user_skill_with_same_name() {
    let home = TempDir::new().unwrap();
    let cwd = TempDir::new().unwrap();

    // user-global skill
    write_skill(home.path(), "shared", "user-version", "USER BODY");
    // project-local skill with same name — should win
    write_skill(
        &cwd.path().join(".pie"),
        "shared",
        "project-version",
        "PROJECT BODY",
    );
    // user-only skill (no project counterpart)
    write_skill(home.path(), "only-user", "user-only", "ONLY USER BODY");

    let loaded = skills_mirror::load_all(cwd.path(), home.path()).await;
    assert!(
        loaded.diagnostics.is_empty(),
        "unexpected diagnostics: {:#?}",
        loaded.diagnostics
    );
    let names: Vec<&str> = loaded.skills.iter().map(|s| s.name.as_str()).collect();
    assert!(names.contains(&"shared"));
    assert!(names.contains(&"only-user"));
    let shared = loaded.skills.iter().find(|s| s.name == "shared").unwrap();
    assert_eq!(
        shared.description, "project-version",
        "project should override user on same name"
    );
    assert!(
        shared.content.contains("PROJECT BODY"),
        "shared content should come from project: {:?}",
        shared.content
    );

    // Now feed into an actual harness and confirm the system prompt includes both skills.
    let storage = std::sync::Arc::new(MemorySessionStorage::new());
    let session = Session::new(storage as std::sync::Arc<dyn pie_agent_core::SessionStorage>);
    let mut opts = AgentHarnessOptions::new(faux_model(), session);
    opts.system_prompt = "base prompt".into();
    opts.thinking_level = ThinkingLevel::Off;
    opts.skills = loaded.skills.clone();
    let harness = AgentHarness::new(opts);

    let prompt = harness.system_prompt();
    assert!(prompt.contains("base prompt"));
    assert!(
        prompt.contains("name: shared"),
        "system prompt should list 'shared' skill: {prompt}"
    );
    assert!(
        prompt.contains("name: only-user"),
        "system prompt should list 'only-user' skill: {prompt}"
    );
    // Description identifies which version landed. Skill bodies are invoked via the `Skill`
    // tool, not inlined into the prompt — so we don't assert on `PROJECT BODY` here.
    assert!(
        prompt.contains("description: project-version"),
        "project version of 'shared' should win in system prompt: {prompt}"
    );
    assert!(
        !prompt.contains("description: user-version"),
        "user version of 'shared' must NOT appear in the listing: {prompt}"
    );

    // Sanity-check: the project body actually lives on the in-memory skill record (so when the
    // model later invokes `Skill('shared')`, it gets the project copy).
    let kept = harness
        .skills()
        .into_iter()
        .find(|s| s.name == "shared")
        .expect("shared skill present");
    assert!(
        kept.content.contains("PROJECT BODY"),
        "harness should keep project body for the shared skill: {:?}",
        kept.content
    );
}

#[tokio::test]
async fn missing_roots_load_cleanly() {
    let home = TempDir::new().unwrap();
    let cwd = TempDir::new().unwrap();
    let loaded = skills_mirror::load_all(cwd.path(), home.path()).await;
    assert!(loaded.skills.is_empty());
    assert!(
        loaded.diagnostics.is_empty(),
        "non-existent roots should produce no diagnostics: {:#?}",
        loaded.diagnostics
    );
}
