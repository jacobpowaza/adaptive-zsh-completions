# Provider development

Providers exist only for values that generic schemas cannot know: repository branches, remote repositories, running containers, project scripts, and similar dynamic state.

Implement `Provider` in `src/providers/`:

```rust
pub trait Provider {
    fn name(&self) -> &'static str;
    fn matches(&self, context: &ProviderContext<'_>) -> bool;
    fn complete(&self, context: &ProviderContext<'_>) -> anyhow::Result<Vec<Candidate>>;
}
```

`matches` must be cheap and precise. `complete` returns data; it does not rank or render. Use `Source::Dynamic`, give candidates concise descriptions, and leave deduplication and ranking to the engine.

For local subprocesses, use `safety::run_informational`, a strict timeout, closed stdin, and explicit read-only arguments. Never pass arbitrary buffer fragments as a command or subcommand unless a validated native protocol defines that position.

For network providers:

- use an official HTTPS API;
- apply a 2–4 second timeout and 2 MiB maximum response;
- cache by the least-sensitive stable key with a short TTL;
- return cached results in offline mode and return no candidates on an offline miss;
- pass every remote insertion through `sanitize_remote`;
- keep credentials in process memory and never include them in cache keys, errors, or logs;
- test with a local mock server, including offline reuse and hostile candidate strings.

The built-in forge adapter supports GitHub, GitLab, Codeberg/Forgejo, Gitea.com, and Bitbucket Cloud. Each host has a small response normalizer over the same cache, ranking, timeout, and insertion pipeline; command parsing remains generic.

The adapters follow the official [GitHub REST](https://docs.github.com/en/rest/repos/repos), [GitLab Projects](https://docs.gitlab.com/api/projects/#list-all-personal-projects-for-a-user), [Gitea API](https://docs.gitea.com/api/), and [Bitbucket repositories](https://developer.atlassian.com/cloud/bitbucket/rest/api-group-repositories/) contracts.

## External schema source (carapace)

`src/carapace.rs` is deliberately not a `Provider`: it is a cross-command schema layer that runs once per query for *any* command with a registered completer, not one narrow dynamic value. It speaks carapace's own interchange protocol (`carapace <completer> export <args...> <current>` returning the JSON export format) through the same safety rules as everything else — bounded timeout, closed stdin, no shell interpolation, `sanitize_remote` on every returned string — and caches answers under a fingerprint of the carapace binary so upgrades invalidate automatically.

Its candidates join the provider pool before ranking, tagged `Source::Carapace`. Where Carapace answered, help discovery is skipped for that position; an explicit Tab listing still merges both. Keep new integration work in this shape: exact machine protocols over parsed prose, cached aggressively, degrading to silence.

Add the provider in `providers::candidates`, unit-test parsing, and add an integration test that invokes the public query protocol.

The Git provider is the reference for repo-aware value slots: it completes branches for `checkout`/`switch`, remotes then branches for `push`, local heads minus the checked-out branch for `branch -d` (never suggest a deletion that would be refused), recent commits by hash with subjects in the display for `cherry-pick`, and worktree-aware path/branch slots for `worktree add`. Structured facts come from plumbing commands (`for-each-ref`, `worktree list --porcelain`) so suggestions stay valid; ranking makes frequently visited branches lead.
