# Research: gitbutler

> Collected 2026-10-09 by research agents for neomail's design. Facts carry a confidence
> level and a source; re-verify anything time-sensitive before relying on it.

**Status (2026-10-09):** the latest stable release is **0.22.3** (2026-08-29) and the latest nightly is 0.5.2220. Master already has unreleased breaking changes (see Pitfalls). Note: this research didn't use a browser, so closing Chromium had no effect on it.

### Installing on Arch/CachyOS
- **No official Arch package; AUR only:**
  - `gitbutler-bin` 0.22.3 repackages the official .deb (GUI, `/usr/bin/but`, webkit2gtk-4.1/gtk3 deps).
  - `gitbutler` 0.21.2 builds from source and lags behind.
  - `gitbutler-cli-bin` is stuck at **0.19.13** (May 2026), from before the command revamp. Avoid it.
- **CLI only, no desktop app needed:** `curl -fsSL https://gitbutler.com/install.sh | sh`.
  - It fetches a signed installer that puts `but` in `~/.local/bin/but`.
  - It offers PATH and completion setup for fish, zsh and bash, plus an agent-setup prompt. `GITBUTLER_NONINTERACTIVE=1` skips the prompts; `sh -s nightly` or `sh -s 0.22.3` picks a channel.
  - On Linux, `but update install [release|nightly|<ver>]` updates only the CLI.
  - The binary needs glibc ≥ 2.35. You can also build from source with `cargo build --release -p but`.
- Don't install both `/usr/bin/but` (AUR) and `~/.local/bin/but` (installer).

### How the workspace works
- `but setup` does four things:
  - registers the repo;
  - sets the target to the remote's HEAD, adding a `gb-local` remote if there's no push remote;
  - checks out `gitbutler/workspace`, a merge commit GitButler rebuilds from all applied branches;
  - installs pre-commit and post-checkout guard hooks.
- `but setup --init` creates a new repo. There is no `but init`. `but teardown` returns you to plain git.
- **Branch types:** parallel branches are independent and all applied in one working dir. Stacked branches are created with `--above`/`--below`.
- **IDs:** uncommitted changes live in the "uncommitted area" (`zz` in 0.22.x). Commits use stable change-ID prefixes. Files and hunks use short IDs shown by `but status -fv` and `but diff` (hunks look like `file:hunk`).

### Core commands (0.22.x)
- **Inspect:** `but status [-f -v -u]`, `but diff [id]`, `but show <id>`, `but branch list|show`, `but tui` (vim keys).
- **Commit:**
  - `but commit -b <branch> -m "msg" <id> <id>` creates the branch if needed.
  - With no IDs it commits **all** uncommitted changes.
  - If more than one stack is applied, you must pass a target.
  - `but branch new <n> [--above <b>]`
- **History edits:** `but amend -t <commit> <ids>`, `but squash <src> -t <tgt>`, `but uncommit`, `but move <c> --below <c>`, `but move <child> --above <parent>`, `--unstack`, `but reword <id> -m`, `but absorb`, `but discard`.
- **Removed commands:**
  - `rub`, `stage` and `unstage` were removed in 0.22.0 (2026-07-30); `mark` and `unmark` in 0.20.4.
  - The binary prints a hint when it sees old syntax.
- **Remote and forge:**
  - Push and pull: `but push <branch>`, `but pull [--check]`.
  - PRs: authenticate with `but config forge auth`, then `but pr new <branch> -t [--draft]`. `but mr` is an alias; for a stack, name the top branch.
  - Skip the PR entirely: `but land <branch> --yes`.
- **Recovery:** `but undo`, `but redo`, `but oplog`, `but oplog snapshot -m`, `but oplog restore <sha>`.
- **Scripting:** global `--json`, `--status-after`, `-C <path>`, and the `BUT_OUTPUT_FORMAT` env var. Any `but-<cmd>` on PATH works as a subcommand.

### Multiple AI agents in one working dir
- **The old integrations are gone:**
  - Claude Code hooks (`but claude pre-tool|post-tool|stop`), Cursor hooks, the old `but mcp` server and the in-app Claude GUI were deleted in PR #13850 (merged 2026-05-19) and are absent from 0.20.0 onward.
  - Their docs were removed on 2026-06-02.
  - The README of the `gitbutlerapp/claude` plugin now says its hooks are no longer recommended.
- **No automatic per-session branch assignment:** since 0.22.0 the CLI also ignores desktop change assignments. The docs say outright that "branch routing is the agent's job".
- **Supported setup is skill plus instructions:**
  - `but agent setup` is a wizard (since 0.20.4). It installs the skill to `.claude/skills/gitbutler/` (or `~/.claude/skills/...`).
  - It writes a managed `## Version control` block to `CLAUDE.md` (or the global `~/.claude/rules/gitbutler.md`).
  - The block says: assume multiple agents, use one dedicated branch per session, commit only your own changes, never touch another agent's work, and don't push unless asked.
- **Per-agent loop:** `but diff`, then `but commit -b <its-branch> -m … <its ids>`.
- After every CLI update, run `but skill check --update`.
- A hidden, undocumented `but mcp serve` exists. It only exposes workspace views, review cards and PR refresh/mark-ready actions; it does not coordinate agents.

### Git and auth interplay
- Read-only git commands are fine.
- `git commit` on the workspace is blocked by the hook.
- `git checkout` or `git switch` to a normal branch leaves GitButler mode.
- The hooks respect `core.hooksPath` and keep your originals as `*-user`. Master switches to config-based hooks on git ≥ 2.54 (you have 2.55).
- Push and fetch use your system git credentials (SSH agent or credential helper).
- PR API calls use tokens from `but config forge auth`:
  - GitHub: device-flow OAuth, PAT, or Enterprise; GitLab and Bitbucket are also supported.
  - Tokens are stored in the Linux Secret Service keyring, so a keyring daemon must be running.
- Use `but pr`, not `gh pr create`, for stacked PRs.

### Pitfalls
- **No runtime isolation:** agents share files, `target/` and each other's half-finished edits (stated in the docs). Edits from two agents in the same hunk can't be split.
- **Concurrent `but` writes:**
  - Issue #13028 (open): concurrent writes can fail with SQLite "database is locked", and `--status-after` timeouts leave it unclear whether the write happened.
  - Issue #16224 (open): hunk IDs shift when other writers change the workspace.
- **Commands that hit the whole repo** when run without explicit IDs or branch names:
  - bare `commit`, `absorb` and `discard`;
  - non-interactive bare `push`, which pushes every branch;
  - `undo` and `pull`.
- **Version drift:**
  - On master (merged after 0.22.3): `zz` becomes `@` (PR #15678), `land` becomes `merge` with `land` kept as an alias (PR #15765), and there is a new `but split`.
  - Blogs and LLM training data still teach the removed `rub`, `stage`, `--changes` and `but claude` forms.
  - The tutorial mentions `but commit --ai`, but the 0.22.3 parser has no such flag.
- **Telemetry and network:** metrics and error reporting are on by default, and GitButler auto-fetches every 15 minutes.
- **Global agent setup leaks:** choosing global scope in `but agent setup` puts the block in every repo (issue #15681).

## Key facts

- **[high]** Latest stable GitButler is 0.22.3 (published 2026-08-29); nightly channel reports 0.5.2220 (2026-10-09). Linux standalone CLI URL pattern: https://releases.gitbutler.com/releases/release/0.22.3-3234/linux/x86_64/but — <https://github.com/gitbutlerapp/gitbutler/releases/tag/release/0.22.3>
- **[high]** No official Arch repo package. AUR: gitbutler-bin 0.22.3 (repackages official .deb; depends webkit2gtk-4.1, gtk3, libayatana-appindicator), gitbutler 0.21.2 (source build with packaged-but-distribution,disable-auto-updates features), gitbutler-cli-bin 0.19.13 (CLI-only, last updated 2026-05-21, predates July 2026 command revamp). — <https://aur.archlinux.org/packages/gitbutler-bin>
- **[high]** The official 0.22.3 .deb contains /usr/bin/but, /usr/bin/gitbutler-tauri and /usr/bin/gitbutler-git-askpass (verified by listing the package); deb/rpm have bundled the CLI since 0.19.6. — <https://blog.gitbutler.com/but-for-linux>
- **[high]** Official CLI install: `curl -fsSL https://gitbutler.com/install.sh | sh`. The bootstrap downloads a signed but-installer that on Linux installs only the CLI to ~/.local/bin/but (signature-verified, with backup/rollback), and offers PATH/completion lines for bash/zsh/fish (fish: `fish_add_path $HOME/.local/bin`, `but completions fish | source`). `GITBUTLER_NONINTERACTIVE=1` skips shell-config and agent-setup prompts; `sh -s nightly` or `sh -s <version>` selects channel. — <https://github.com/gitbutlerapp/gitbutler/blob/master/crates/but-installer/src/install_linux.rs>
- **[high]** The desktop app is not required: `but update install [nightly|release|<version>]` installs/updates only the CLI on Linux; the CLI binary links glibc (2.35 symbols); build from source with `cargo build --release -p but`. — <https://docs.gitbutler.com/commands/but-update>
- **[high]** `but setup` registers the repo in the global project registry, switches to gitbutler/workspace, sets target to the remote HEAD, adds a gb-local remote if no push remote exists, and installs pre-commit/post-checkout guard hooks; `but setup --init` creates a repo with an empty commit; there is no `but init` command; `but teardown [--checkout-to <branch>]` snapshots, checks out a branch and cherry-picks dangling commits. — <https://docs.gitbutler.com/commands/but-setup>
- **[high]** gitbutler/workspace is a continually rebuilt merge commit of all applied branches; git status/diff show the union; `git commit` on it is rejected by the pre-commit hook with GITBUTLER_ERROR; checking out a normal branch exits GitButler mode via the post-checkout hook. — <https://docs.gitbutler.com/workspace-branch>
- **[high]** 0.22.0 (2026-07-30) overhauled commit/amend/squash/discard/move and removed `rub`, `stage`, `unstage` and `commit --only`; the CLI no longer reads desktop change assignments (all changes are 'uncommitted'); `--json` is the machine-readable flag; history edits refuse merged commits without `--allow-merged`. `mark`/`unmark` were removed in 0.20.4. — <https://github.com/gitbutlerapp/gitbutler/releases/tag/release/0.22.0>
- **[high]** Modern commit syntax: `but commit -b <branch> -m "msg" <id> <id>` (creates branch if missing; no IDs commits ALL uncommitted changes; with more than one applied stack an untargeted commit is refused; with one stack it lands on the stack's top branch). Targets: -b/--branch, -A/--above, -B/--below; also --empty, -i, --no-message. — <https://docs.gitbutler.com/commands/but-commit>
- **[high]** The Claude Code hooks (`but claude pre-tool|post-tool|stop`), Cursor hooks (`but cursor after-edit|stop`), the but-claude/but-cursor crates and the in-app Claude Code GUI were deleted by PR #13850 (merged 2026-05-19); first stable without them is 0.20.0 (2026-06-03, 'superseded by the but skill & workflow'). — <https://github.com/gitbutlerapp/gitbutler/pull/13850>
- **[high]** Docs for Claude Code hooks, Cursor hooks, the MCP server and 'coding agents' were removed from docs.gitbutler.com on 2026-06-02 ('Remove superseded AI integration docs'); the gitbutlerapp/claude plugin README now says hooks are no longer recommended and points to `but skill install --global`. — <https://github.com/gitbutlerapp/claude>
- **[high]** Current agent setup: `but agent setup` (added 0.20.4, 2026-06-26) installs the skill (Claude Code: .claude/skills/gitbutler/ in repo or ~/.claude/skills/gitbutler/) and writes a managed '## Version control' block (repo CLAUDE.md or global ~/.claude/rules/gitbutler.md); `but agent setup --print` prints the text; `but skill check --update` / `but skill install --detect` refresh skills. — <https://docs.gitbutler.com/ai-agents/getting-started>
- **[high]** Parallel agents need no worktrees or pre-created branches, but there is no automatic per-session assignment: 'The branch routing is the agent's job.' Default steering says: assume multiple agents, do not modify another agent's work, use a dedicated GitButler branch per session, commit only that session's changes, do not push/open PRs unless asked. — <https://docs.gitbutler.com/ai-agents/parallel-agents>
- **[high]** Parallel GitButler branches are not runtime isolation: agents share filesystem, dependency installs, generated files and app state; docs recommend worktrees for incompatible checkout/runtime state; same-hunk edits cannot be split by ID. — <https://docs.gitbutler.com/ai-agents/parallel-agents>
- **[medium]** A hidden, undocumented `but mcp serve` (stdio) exists in 0.22.3 exposing gitbutler_workspace, gitbutler_commit_details, gitbutler_branch_details, gitbutler_review_card, gitbutler_refresh_reviews, gitbutler_mark_review_ready with MCP App views; the old `but mcp` (gitbutler_update_branches) is gone. — <https://github.com/gitbutlerapp/gitbutler/blob/release/0.22.3/crates/but/src/args/mcp.rs>
- **[high]** Concurrency: issue #13028 (open) reports concurrent `but` mutations in one working copy failing with SQLite 'database is locked' and `--status-after` timeouts after the mutation applied; PR #13094 (2026-03-30) added an inter-process file lock as a partial fix; issue #16224 (open, 2026-10-04) notes hunk IDs are snapshot-scoped and shift under concurrent writers. — <https://github.com/gitbutlerapp/gitbutler/issues/13028>
- **[high]** Push/fetch use the system git executable and its auth (SSH agent, credential helper); GitButler no longer has its own SSH/credential modes. — <https://docs.gitbutler.com/troubleshooting/fetch-push>
- **[high]** Forge auth: `but config forge auth` is interactive (GitHub: Device flow OAuth, PAT, or Enterprise; GitLab PAT/self-hosted; Bitbucket API token). GitHub PAT needs Metadata read + Pull requests read/write. On Linux release builds tokens go to the system keychain via Secret Service (org.freedesktop.secrets); missing keyring yields 'System keychain is not available'. — <https://docs.gitbutler.com/features/forge-integration/github-integration>
- **[high]** `but pr new <branch>` pushes first (force-push with protection by default) and for stacks pushes ancestors and creates PRs bottom-up; use `but pr`, not `gh pr create`, for stacks; `but land <branch> --yes` lands directly on the target (0.21.0+). — <https://docs.gitbutler.com/commands/but-pr>
- **[high]** Non-interactive bare `but push` pushes every branch with unpushed commits; `--with-force` defaults to true (with force-push protection). — <https://docs.gitbutler.com/commands/but-push>
- **[high]** Managed hooks respect core.hooksPath and preserve existing hooks as pre-commit-user/post-checkout-user (0.22.3); master (PR #14698, merged 2026-08-31, after 0.22.3) registers config-based hooks on git >= 2.54 under <git-common-dir>/gitbutler/hooks with opt-out `git config gitbutler.installHooks false`. — <https://github.com/gitbutlerapp/gitbutler/pull/14698>
- **[high]** Unreleased breaking changes on master: uncommitted area ID `zz` -> `@` (PR #15678, 2026-09-01), `but land` renamed to `but merge` with `land` alias (PR #15765, 2026-09-07), new `but split`, `but worktree new` behind experimental worktreeManipulation flag. — <https://github.com/gitbutlerapp/gitbutler/pull/15678>
- **[high]** Telemetry: appMetricsEnabled and appErrorReportingEnabled default to true (settings at ~/.config/gitbutler/settings.json); auto-fetch every 15 minutes (set fetch.autoFetchIntervalMinutes to -1 to disable); `but config metrics disable` turns metrics off. — <https://github.com/gitbutlerapp/gitbutler/blob/release/0.22.3/crates/but-settings/assets/defaults.jsonc>
- **[high]** The CLI auto-detects agents via env vars (e.g. CLAUDECODE, CODEX_THREAD_ID, CURSOR_AGENT) and tailors output; BUT_OUTPUT_FORMAT=json|human; global -C <path>; unknown subcommands run `but-<cmd>` from PATH; default aliases st, stf, c, m, s, wt. — <https://github.com/gitbutlerapp/gitbutler/blob/master/crates/but/src/utils/detect_agent.rs>
- **[high]** Per-project GitButler state is stored in .git/gitbutler (SQLite); path overridable with `git config gitbutler.storagePath`. — <https://docs.gitbutler.com/development/debugging>
- **[medium]** `but agent setup` with global scope writes a ~42-line block that loads in every repository, including non-GitButler ones (issue #15681, open). — <https://github.com/gitbutlerapp/gitbutler/issues/15681>
- **[medium]** Docs tutorial mentions `but commit --ai`, but the 0.22.3 `but commit` reference/argument parser has no --ai flag (stale doc). — <https://docs.gitbutler.com/cli-guides/cli-tutorial/ai-stuff>

## Recommendations

- Install the CLI on CachyOS with the official installer: `curl -fsSL https://gitbutler.com/install.sh | sh`. In fish, make sure ~/.config/fish/config.fish contains `fish_add_path $HOME/.local/bin` and `but completions fish | source`. If you also want the GUI, run `paru -S gitbutler-bin` instead. Avoid `gitbutler-cli-bin` (0.19.13), and don't install both.
- Pin to stable (`but update install release`) and run `but skill check --update` after every update. Treat `zz`, `land` and other 0.22.x syntax as temporary: the next release brings `@`, `merge` and config-based hooks, so keep scripts version-aware.
- Bootstrap the repo with the GitHub remote in place first: `git init -b main`, `git remote add origin git@github.com:<you>/neomail.git`, an initial commit and `git push -u origin main`. Then run `but setup` and `but config target origin/main`. Doing it in this order avoids the gb-local target. Then run `but config metrics disable`.
- Auth: confirm `git fetch` works over SSH. You (not an agent) then run `but config forge auth` and pick GitHub Device flow. Make sure a Secret Service keyring is running (gnome-keyring or KeePassXC).
- Agent setup: run `but agent setup`, choose Claude Code and the 'Just this project' scope. This installs the skill to .claude/skills/gitbutler/ and adds the steering block to CLAUDE.md. Non-interactive alternative: `but skill install --path .claude/skills/gitbutler`, then copy the output of `but agent setup --print` into CLAUDE.md. Remove any old `but claude pre-tool/post-tool/stop` hooks from Claude settings and don't install the gitbutlerapp/claude plugin.
- Our suggestion (not GitButler's): enforce the rules with Claude Code permissions in .claude/settings.json. Deny raw git writes such as `Bash(git commit:*)`, `Bash(git push:*)`, `Bash(git checkout:*)`, `Bash(git switch:*)`, `Bash(git stash:*)`, `Bash(git rebase:*)` and `Bash(git reset:*)`, and allow `Bash(but:*)`. GitButler's docs say its instructions steer agents but are not access controls.
- Per-agent loop: `but diff`, then right away `but commit -b agent/<task> -m "msg" <id> <id>`, re-reading IDs right before each commit. Never let agents run bare `but commit`, `but absorb`, `but discard`, `but push`, `but undo` or `but pull`. Before an update, run `but pull --check` and have a human approve it. Take `but oplog snapshot -m "<reason>"` before big history edits and recover with `but oplog restore <sha>`.
- Review and publish as the human: inspect with `but tui` or `but status -fv` and `but branch show <b> --files`. Publish with `but pr new <branch> -t --draft`. For dependent work, stack with `but move <child> --above <parent>` and publish once with `but pr new <top-branch> -t`.
- Rust-specific: parallel agents share one cargo `target/` lock and break each other's builds with half-finished edits. Give each agent its own `CARGO_TARGET_DIR`, split work along cargo-workspace crate boundaries, and use plain git worktrees for agents that need a stable build or test state.
- If you hit 'database is locked' (issue #13028), serialize `but` writes, e.g. wrap them in bash as `flock /tmp/neomail-but.lock but commit ...`. This is our workaround, not an official one.

## Open questions

- When will the next stable release (0.23?) ship the master changes (`zz` to `@`, `land` to `merge` with a `land` alias, `but split`, config-based hooks, `but worktree new`)? No date is announced.
- Does `but commit` from the CLI run the repo's pre-commit and commit-msg hooks? The desktop app has a per-project 'Run Git hooks' toggle, and the old `--no-hooks` commit flag was removed with no modern equivalent. Not verified.
- How bad is concurrent-write locking in 0.22.3 in practice? Issue #13028 is still open and only a partial inter-process lock has landed. Needs an empirical test with 2–3 agents.
- Is the hidden `but mcp serve` meant for public use? It's undocumented and could change or disappear.
- Can forge auth run non-interactively (e.g. a PAT from stdin or an env var) in a headless or agent sandbox? The code shows interactive prompts only.
- Does `but` reuse the `gh` CLI's GitHub auth? No evidence found; it seems to keep its own tokens in the keychain.
- Will the AUR maintainers update `gitbutler-cli-bin` (stuck at 0.19.13) and `gitbutler` (0.21.2)?
