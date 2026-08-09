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

Add the provider in `providers::candidates`, unit-test parsing, and add an integration test that invokes the public query protocol.
