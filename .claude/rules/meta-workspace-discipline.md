# Meta Workspace Discipline

You are working in a **meta-repo** — multiple independent git repositories managed together.

## Required Behaviors

1. **Use `meta git` for cross-repo operations** — NOT raw `git`
   - `meta git status` shows all repos at once
   - `meta git commit -m "msg"` commits in all dirty repos

2. **Use `meta exec` for cross-repo commands** — NOT `cd <repo> && cmd`
   - `meta exec -- npm install` runs in all repos
   - `meta --include X,Y exec -- cmd` targets specific repos

3. **Check scope before committing**
   - Run `meta git status` to see which repos have changes
   - Verify you intend to commit in all listed repos

4. **Target precisely with filters**
   - `--include repo1,repo2` — only these repos
   - `--exclude repo3` — skip this repo
   - `--tag backend` — only repos with this tag

5. **Use local-first validation by default**
   - Heavy validation belongs in the local checkout or self-hosted runner first
   - GitHub Actions should be the smallest useful receipt for the pushed commit
   - Full cloud CI requires release/manual/label/schedule/self-hosted/operator-task intent
   - Token-consuming AI/Codex/OpenAI checks must never run from default PR/push CI
