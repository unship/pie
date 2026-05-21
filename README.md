# pie

`pie` is a terminal coding agent. Run it inside a project, ask it to inspect files, make edits,
run shell commands, remember preferences, and continue previous sessions.

## Install / build

```bash
git clone https://github.com/c4pt0r/pie.git
cd pie
cargo build --release
```

The CLI binary will be at `./target/release/pie`.

## Configure a model

`pie` auto-detects the first available provider credential. Set an API key before starting:

```bash
export ANTHROPIC_API_KEY=sk-ant-...
# or: OPENAI_API_KEY, OPENROUTER_API_KEY, GROQ_API_KEY,
#     MISTRAL_API_KEY, GEMINI_API_KEY, GOOGLE_API_KEY
```

You can also store a key from inside `pie`:

```text
/login anthropic sk-ant-...
```

## Quick start

```bash
# Start in the current project
./target/release/pie

# Choose a specific provider/model
./target/release/pie --provider anthropic --model claude-haiku-4-5

# Enable extended thinking where supported
./target/release/pie --thinking high

# Resume the most recent session for this project
./target/release/pie --resume
```

Once the REPL opens, type a request such as:

```text
summarize this repository
fix the failing tests
add a README example and run the relevant checks
```

## Useful commands

Inside `pie`, slash commands control the session:

| Command | What it does |
|---------|--------------|
| `/help` | Show all commands |
| `/model [provider:model-id]` | Show or switch model |
| `/thinking [off|minimal|low|medium|high|xhigh]` | Show or set thinking level |
| `/sessions` | List sessions for the current project |
| `/save [path]` | Export the transcript to Markdown |
| `/compact [instructions]` | Compact long context |
| `/undo` | Remove the most recent user/assistant turn |
| `/cost` | Show token and cost totals |
| `/login <provider> <api-key>` | Store an API key |
| `/logout <provider>` | Remove a stored API key |
| `/quit` | Exit |

CLI helpers:

```bash
./target/release/pie --help
./target/release/pie --list-sessions
./target/release/pie --list-all-sessions
./target/release/pie --delete-session <id>
./target/release/pie --image screenshot.png
```

## What pie can do

The agent has tools for common coding workflows:

- read, write, and edit files
- list files and search with grep/find
- run shell commands
- manage persistent memory
- delegate focused sub-tasks
- resume JSONL-backed sessions per project
- attach images to the first prompt with `--image`

## Files and storage

By default, `pie` stores local state under `~/.pie`:

| Path | What |
|------|------|
| `~/.pie/sessions/<cwd-hash>/<uuidv7>.jsonl` | Session history for each project |
| `~/.pie/memory/*.md` | Cross-session memory injected into future sessions |
| `~/.pie/auth.json` | Stored API keys from `/login` |
| `~/.pie/history` | Prompt history |

Set `PIE_DIR` to use a different base directory.

## Development

```bash
cargo build --workspace
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all --check
```

## Credits

`pie` is a Rust rewrite of the original [`pi`](https://github.com/earendil-works/pi) project by earendil-works.

## License

[MIT](LICENSE)
