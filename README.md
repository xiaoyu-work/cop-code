# Cop Code

A terminal-based AI coding agent powered by your **GitHub Copilot subscription**. No separate API key needed — just `claw login copilot` and go.

Forked from [Claw Code](https://github.com/ultraworkers/claw-code) (a Rust reimplementation of Claude Code). The primary addition is **GitHub Copilot OAuth support**, so anyone with a Copilot subscription can use a Claude-quality coding agent directly from the terminal.

## Install

```bash
curl -fsSL https://raw.githubusercontent.com/xiaoyu-work/cop-code/main/install.sh | bash
```

Or build from source:

```bash
cd rust
cargo build --release --bin claw
```

## Usage

### GitHub Copilot (recommended)

```bash
# One-time login via GitHub device flow
claw login copilot

# Launch
claw --provider copilot
```

### Anthropic

```bash
export ANTHROPIC_API_KEY="sk-ant-..."
claw
```

### OpenAI-compatible

```bash
export OPENAI_API_KEY="sk-..."
claw --provider openai --model gpt-4o
```

## Features

- 🤖 Interactive REPL + one-shot prompt mode
- 🔧 Built-in tools: bash, read/write files, search, web fetch
- 🔑 GitHub Copilot OAuth, Anthropic API key/OAuth, OpenAI-compatible
- 🧩 Plugin system & MCP support
- 💾 Session persistence & resume

## Acknowledgements

This project is based on [Claw Code](https://github.com/ultraworkers/claw-code) by the UltraWorkers community, which is itself a Rust reimplementation inspired by Anthropic's Claude Code.

This project is **not affiliated with, endorsed by, or maintained by Anthropic or GitHub**.
