# Ollama Provider Discipline

When operating in this workspace under the Ollama provider:

1. **Prioritize Local Knowledge**: Use `meta context` and `git kb recall` to understand the workspace instead of asking the user for basic info.
2. **Compress Output**: Always use `rtk` (if available) when running commands that produce large logs (e.g., `meta exec -- cargo build | rtk`).
3. **Chunk Large Tasks**: Break down multi-repo refactors into smaller, verifiable steps to accommodate local model context limits.
4. **Tool Verification**: Confirm tool availability (like `meta`, `rtk`, `ollama`) before suggesting complex workflows.
