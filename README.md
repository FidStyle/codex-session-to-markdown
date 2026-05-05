# codex-prompts

Extract user prompts from Codex session JSONL files.

## Usage

```bash
cargo build --release
./target/release/codex-prompts ~/.codex/sessions/2026/04/23/rollout-example.jsonl
./target/release/codex-prompts --with-context ~/.codex/sessions/2026/04/23/rollout-example.jsonl
```

Default output is only the user prompt text, separated by blank lines. `--with-context` adds the nearest previous assistant `response_item` message before each prompt.

The parser uses `response_item.payload.type == "message"` with `role == "user"` or `role == "assistant"`. It does not use `event_msg.payload.last_agent_message`.
