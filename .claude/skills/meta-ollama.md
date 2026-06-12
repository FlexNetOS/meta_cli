# Ollama Local AI Skill

This workspace is optimized for local AI development using **Ollama**.

## Why Ollama?

Local execution provides:
- **Zero latency**: Instant tool execution and analysis.
- **Privacy**: Code never leaves your machine.
- **Offline access**: Keep working without an internet connection.
- **Cost-effective**: Run large-scale refactors without token costs.

## Interaction Patterns

When running through Ollama, follow these patterns:

1. **Be Concise**: Large context windows can be slower. Focus on the most relevant files.
2. **Local First**: Use local documentation and `meta` context tools rather than web search.
3. **RTK Integration**: Use `rtk` (Rust Token Killer) to compress tool outputs and save memory/context.

## Verification

Ensure Ollama is running:

```bash
ollama list
```

If you need to start a specific model:

```bash
ollama run llama3
```

## IDE Integration (Rust Rover)

To use Ollama in Rust Rover:
1. Open **Settings** (`Ctrl+Alt+S`).
2. Go to **Tools** -> **AI Assistant** (or the specific plugin like Continue/Codeium).
3. Set the **Provider** to `Ollama`.
4. Point the API URL to `http://localhost:11434`.
