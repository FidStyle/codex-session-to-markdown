# codex-session-to-markdown

Convert Codex session JSONL files into a readable prompt transcript.

## Install

Install from this repository:

```bash
cargo install --path .
```

This installs the `codex-session-to-markdown` command into Cargo's bin directory, usually `~/.cargo/bin`. Make sure that directory is on your `PATH`.

You can also build a release binary manually:

```bash
cargo build --release
sudo install -m 755 target/release/codex-session-to-markdown /usr/local/bin/
```

## Usage

Pass either a full Codex session `.jsonl` path or the complete session UUID from the end of the rollout file name:

```bash
codex-session-to-markdown 019dfa57-dbd2-7f61-84c4-0c468f27dd1a
codex-session-to-markdown ~/.codex/sessions/2026/04/23/rollout-example.jsonl
codex-session-to-markdown --with-context 019dfa57-dbd2-7f61-84c4-0c468f27dd1a
```

By default it outputs user prompts separated by blank lines. `--with-context` includes the nearest previous assistant reply. `--no-dedupe` keeps duplicate or superseded prompts.
