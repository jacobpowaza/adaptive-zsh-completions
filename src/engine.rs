use crate::{
    cache::Cache,
    config::Config,
    discovery::Discoverer,
    history::{self, HistoryDb, Usage},
    model::{Candidate, CandidateKind, ItemKind, QueryContext, QueryResponse, Source},
    native::native_candidates,
    parser::parse_context,
    priors::{self, Scope},
    providers::{self, ProviderContext},
    ranking::{rank, rank_for_list},
    safety::sanitize_remote,
};
use anyhow::Result;
use std::{
    collections::HashMap,
    path::Path,
    time::{SystemTime, UNIX_EPOCH},
};

/// A confident personal prediction must clear both bars: enough decayed
/// recent use to be a habit (not a one-off), and enough share of everything
/// plausible here that guessing it outright is usually right.
const DOMINANCE_MIN_WEIGHT: f64 = 2.0;
const DOMINANCE_MIN_SHARE: f64 = 0.6;

pub struct Engine {
    pub cache: Cache,
    pub config: Config,
}
impl Engine {
    pub fn new(cache: Cache, config: Config) -> Self {
        Self { cache, config }
    }
    pub fn query(
        &self,
        buffer: &str,
        cursor: usize,
        cwd: &Path,
        offline: bool,
        prev: Option<&str>,
    ) -> Result<QueryResponse> {
        self.query_inner(buffer, cursor, cwd, offline, false, prev)
    }
    pub fn list(
        &self,
        buffer: &str,
        cursor: usize,
        cwd: &Path,
        offline: bool,
        prev: Option<&str>,
    ) -> Result<QueryResponse> {
        self.query_inner(buffer, cursor, cwd, offline, true, prev)
    }
    fn query_inner(
        &self,
        buffer: &str,
        cursor: usize,
        cwd: &Path,
        offline: bool,
        list: bool,
        prev: Option<&str>,
    ) -> Result<QueryResponse> {
        let context = parse_context(buffer, cursor);
        // What the world in general expects at this exact position, adjusted
        // for the directory we are standing in (`git ` means something else
        // inside a repo than outside one). Empty when this position is not a
        // known subcommand slot -- e.g. after `git commit `, where suggesting
        // git's top-level verbs would be nonsense.
        let scope = Scope::detect(cwd);
        let bare_command = context.tokens.is_empty();
        let prior_key_str = prior_key(&context);
        let prior_tokens = priors::priors().next_tokens(&prior_key_str, scope);

        let mut provider_candidates = providers::candidates(&ProviderContext {
            query: &context,
            cwd,
            cache: &self.cache,
            forge_enabled: self.config.providers.forge.enabled,
            docker_enabled: self.config.providers.docker.enabled,
            offline,
        });
        // The optional carapace layer answers with exact command knowledge
        // (subcommands, flags, enums, positionals) for any tool it has a
        // completer for. Its candidates join the provider pool before the
        // documented-options decision below: where carapace spoke, parsing
        // `--help` would only add noise and latency -- the kubectl case --
        // while an explicit TAB listing still merges both.
        if self.config.carapace.enabled
            && !bare_command
            && context
                .command
                .as_deref()
                .is_some_and(crate::carapace::valid_completer_name)
            && let Some(carapace) = crate::carapace::Carapace::detect()
            && let Some(command) = context.command.as_deref()
        {
            // The completer listing (cached per binary fingerprint for a day)
            // is the cheap gate that keeps unknown commands from paying for a
            // subprocess at every keystroke. When no listing can be had,
            // asking anyway stays the safe default.
            let worth_asking = carapace
                .known_completers(&self.cache)
                .is_none_or(|names| names.iter().any(|name| name == command));
            if worth_asking {
                let answers =
                    carapace.complete(&self.cache, command, &context.args, &context.current, cwd);
                provider_candidates.extend(answers);
            }
        }
        // Providers answer "what is valid *here*" (branches, scripts, hosts).
        // When one spoke, generic help text must not bury its answers -- but
        // a leading "-" is the user explicitly asking for this command's
        // documented options, which providers do not know; and an explicit
        // TAB listing should show every documented choice alongside the
        // dynamic ones.
        let asks_for_options = context.current.starts_with('-');
        let has_contextual_values = !provider_candidates.is_empty() && !asks_for_options;
        let wants_documented_options =
            !has_contextual_values || (list && context.current.is_empty());
        let mut candidates = provider_candidates;

        // ---- persistent usage memory -------------------------------------
        let half_life = self.config.completion.half_life_days;
        let profile = HistoryDb::load().unwrap_or_default();
        let mut usage: HashMap<String, Usage> = HashMap::new();
        let mut rejected_usage: HashMap<String, Usage> = HashMap::new();
        // Decayed weights straight from the learned model for this position,
        // used only by the dominance rule. Kept separate from `usage` so a
        // habit that surfaces through several channels (a token candidate and
        // its full-line chain completion) is counted once, not three times.
        let mut predictions: HashMap<String, f64> = HashMap::new();
        // Echoed to the frontend so accept/reject feedback lands back on this
        // exact position without the shell re-deriving engine internals.
        let context_key = position_key(&context, bare_command, cwd);

        if self.config.completion.history && profile.enabled {
            if bare_command {
                // Nothing (or a first letter) typed: predict whole commands.
                if context.current.is_empty() && self.config.completion.predictive {
                    let dir_keys = history::dir_lookup_keys(cwd);
                    let prev_head = prev.and_then(history::command_head);
                    for (name, weight, last) in
                        profile.command_weights(&dir_keys, prev_head.as_deref(), half_life, 12)
                    {
                        candidates.push(Candidate::new(
                            name.clone(),
                            "predicted from your usage",
                            Source::History,
                        ));
                        predictions.insert(name.clone(), weight);
                        let entry = usage.entry(name).or_insert_with(|| Usage {
                            count: 0,
                            last_used: last,
                        });
                        entry.count = entry.count.saturating_add(weight.max(1.0) as u64);
                        entry.last_used = entry.last_used.max(last);
                    }
                }
                // A typed prefix can also complete to an entire remembered
                // line (`c` -> `claude --dangerously-skip-permissions`).
                if self.config.completion.chain_prediction && !context.current.is_empty() {
                    for (line, seen) in profile.chain_from_head(&context.current, half_life, 8) {
                        usage.insert(line.clone(), seen);
                        candidates.push(Candidate::new(
                            line,
                            "full command from your usage",
                            Source::History,
                        ));
                    }
                }
            } else if let Some(command) = context.command.as_deref() {
                // Trailing space or partial token: the learned next-token
                // table answers instantly, before any discovery runs.
                let exact = exact_path_key(command, &context.args);
                if self.config.completion.predictive {
                    for (token, weight, last) in profile.next_token_weights(&exact, half_life, 16) {
                        predictions.insert(token.clone(), weight);
                        let entry = usage.entry(token.clone()).or_insert_with(|| Usage {
                            count: 0,
                            last_used: last,
                        });
                        entry.count = entry.count.saturating_add(weight.max(1.0) as u64);
                        entry.last_used = entry.last_used.max(last);
                        // Recorded directory habits apply to the slash form
                        // the filesystem provider actually completes.
                        if command == "cd" && !token.ends_with('/') {
                            let mut with_slash = usage[&token].clone();
                            with_slash.count = with_slash.count.saturating_sub(1);
                            usage.insert(format!("{token}/"), with_slash);
                        }
                        candidates.push(Candidate::new(
                            token,
                            "learned from your usage",
                            Source::History,
                        ));
                    }
                }
                // Full remainders of remembered lines (`git clone` already
                // typed, remainder `https://github.com/...`) continue the
                // chain past a single token.
                if self.config.completion.chain_prediction {
                    for (rest, seen) in
                        profile.chain_after_command(&exact, &context.current, half_life, 8)
                    {
                        usage.insert(rest.clone(), seen);
                        candidates.push(Candidate::new(
                            rest,
                            "rest of command from your usage",
                            Source::History,
                        ));
                    }
                }
            }
            // Deliberate acceptance is the strongest kind of usage: fold it
            // into frequency/recency exactly like another real execution.
            if let Some(feedback) = profile.accepted.get(&context_key) {
                for (value, seen) in feedback {
                    let weight = seen.weight(half_life, now_secs());
                    let entry = usage.entry(value.clone()).or_default();
                    entry.count = entry.count.saturating_add(weight.ceil().max(1.0) as u64);
                    entry.last_used = entry.last_used.max(seen.last_used);
                }
            }
            if let Some(rejections) = profile.rejected.get(&context_key) {
                rejected_usage = rejections.clone();
            }
            if bare_command {
                // First token on the line: every recorded command line is a
                // vote for its own name, so a user who lives in `gh` gets
                // `gh` ahead of the global default `git`.
                for (pattern, recorded) in &profile.patterns {
                    if let Some(name) = pattern.split_whitespace().next() {
                        add_usage(&mut usage, name.to_owned(), recorded);
                    }
                }
            }
        }

        // A forge prefix candidate (`https://github.com/`) at an untyped URL
        // slot is canonical: remembered full URLs must not hijack it, no
        // matter how often one repository was cloned. A specific repo is
        // reached by typing its owner, which re-engages the forge provider.
        if context.current.is_empty()
            && candidates
                .iter()
                .any(|c| c.source == Source::Dynamic && c.value.ends_with('/'))
        {
            candidates.retain(|c| !(c.source == Source::History && c.value.contains("://")));
            predictions.retain(|value, _| !value.contains("://"));
        }

        if wants_documented_options && let Some(command) = context.command.as_deref() {
            let discoverer =
                Discoverer::new(&self.cache, self.config.completion.online_docs && !offline);
            if let Ok(root) = discoverer.discover(command, &[], false) {
                let mut native_args = context.args.clone();
                native_args.push(context.current.clone());
                candidates.extend(native_candidates(&root, command, &native_args, cwd));
                let mut schema = root;
                let mut path = Vec::new();
                for argument in &context.args {
                    let declares_command_slot = schema
                        .usage
                        .as_deref()
                        .is_some_and(|usage| usage.to_ascii_lowercase().contains("command"))
                        || schema
                            .items
                            .iter()
                            .any(|item| item.kind == ItemKind::Subcommand);
                    let is_subcommand = schema.items.iter().any(|item| {
                        item.kind == ItemKind::Subcommand && item.names.contains(argument)
                    }) || (declares_command_slot && !argument.starts_with('-'));
                    if is_subcommand {
                        path.push(argument.clone());
                        if let Ok(child) = discoverer.discover(command, &path, false) {
                            schema = child;
                        }
                    }
                }
                for item in schema.items {
                    if item.kind == ItemKind::Positional {
                        continue;
                    }
                    for name in item.names {
                        if !context.tokens.iter().any(|t| t == &name) {
                            let mut c =
                                Candidate::new(name, item.description.clone(), Source::LocalHelp);
                            c.confidence = schema.confidence;
                            if item.kind == ItemKind::Subcommand {
                                c.kind = CandidateKind::Subcommand;
                            }
                            candidates.push(c)
                        }
                    }
                    for value in item.values {
                        candidates.push(Candidate::new(
                            value,
                            item.description.clone(),
                            Source::LocalHelp,
                        ));
                    }
                }
            }
            // Common subcommands some tools never spell out in their own
            // `--help` (plenty of CLIs answer it with prose, a pager, or an
            // error). Filling them in from the prior table keeps such
            // positions useful while discovery catches up elsewhere.
            for token in prior_tokens {
                if !candidates.iter().any(|c| &c.value == token)
                    && !context.tokens.iter().any(|t| t == token)
                {
                    candidates.push(
                        Candidate::new(token.clone(), "common subcommand", Source::Prior)
                            .with_kind(CandidateKind::Subcommand),
                    );
                }
            }
        }
        candidates.retain_mut(|candidate| {
            let Some(value) = sanitize_remote(&candidate.value) else {
                return false;
            };
            let Some(display) = sanitize_remote(&candidate.display) else {
                return false;
            };
            candidate.value = value;
            candidate.display = display;
            candidate.description = sanitize_remote(&candidate.description).unwrap_or_default();
            true
        });
        // Attach the global prior to every candidate, whatever produced it, so
        // ranking sees one comparable signal instead of per-provider nudges.
        // Only positions where a prior actually applies are touched: a branch
        // or a directory that happens to be named "install" is not the
        // Homebrew subcommand.
        for candidate in &mut candidates {
            candidate.prior = if bare_command {
                let prior = priors::priors().command_prior(&candidate.value);
                if candidate.source == Source::History && candidate.value.contains(' ') {
                    // A remembered line inherits the global standing of the
                    // command it starts with -- `claude --fast` deserves
                    // `claude`'s prior, not nothing, just because it is two
                    // words.
                    let head = candidate
                        .value
                        .split_whitespace()
                        .next()
                        .unwrap_or_default();
                    prior.max(priors::priors().command_prior(head))
                } else {
                    prior
                }
            } else if prior_tokens.is_empty() || candidate.source == Source::Filesystem {
                0.0
            } else {
                priors::priors().token_prior(&prior_key_str, &candidate.value, scope)
            };
            // Multi-word history completions carry strictly more information
            // than their head token alone; nudge them so `alpha beta` outranks
            // the redundant bare `alpha`.
            if candidate.source == Source::History && candidate.value.contains(' ') {
                candidate.prior += 0.25;
            }
        }
        let mut candidates = if list {
            rank_for_list(
                candidates,
                &context.current,
                &usage,
                self.config.completion.fuzzy,
                &rejected_usage,
            )
        } else {
            rank(
                candidates,
                &context.current,
                &usage,
                self.config.completion.fuzzy,
                &rejected_usage,
            )
        };
        // When a habitual full-line completion extends the top candidate
        // (`claude` + remembered `claude --dangerously-skip-permissions`)
        // and would itself lead the list without the head token hogging the
        // source-confidence bonus, the line absorbs the bare token in ghost
        // mode: predicting the line the user actually runs is strictly more
        // useful than its first word. The Tab list keeps both.
        if !list && self.config.completion.chain_prediction && candidates.len() > 2 {
            let now = now_secs();
            let top = candidates[0].value.clone();
            let runner_up = candidates[1].score;
            let absorbed = candidates[1..].iter().any(|c| {
                c.source == Source::History
                    && c.value.contains(' ')
                    && c.value.starts_with(&top)
                    && c.score >= runner_up
                    && usage
                        .get(&c.value)
                        .is_some_and(|u| u.weight(half_life, now) >= DOMINANCE_MIN_WEIGHT)
            });
            if absorbed {
                candidates.remove(0);
            }
        }
        // The zsh frontend only auto-ghosts an empty (untyped) token when we
        // hand it back exactly one candidate -- otherwise it waits for the
        // user to type a prefix or press TAB, to avoid inventing tokens the
        // user never asked for. Three situations justify collapsing here:
        //
        // 1. habitual: this user repeats the same next argument here;
        // 2. common_default: the head of the global prior list for a position
        //    with no personal history yet ("install" after `brew `);
        // 3. dominant: the learned next-token model says this continuation
        //    carries most of the recent probability mass at this position --
        //    the proactive `claude ` -> `--dangerously-skip-permissions`
        //    case, which works after only a couple of runs and needs no
        //    documentation at all.
        //
        // Filesystem listings (cd, etc.) are excluded: there the alternatives
        // are always equally real options to browse, not guesses to suppress.
        if !list
            && context.current.is_empty()
            && candidates.len() > 1
            && candidates[0].source != Source::Filesystem
        {
            let habitual = usage
                .get(&candidates[0].value)
                .is_some_and(|u| u.count >= 3)
                && candidates[0].score - candidates[1].score >= 15.0;
            // With no history yet, the same collapse applies when the
            // top-ranked candidate is the head of the prior list for this
            // position ("install" after `brew `, "up" after `docker compose `,
            // "status" after `git ` inside a repo). Personal usage still wins
            // first: if the user's habits reranked something else to the top,
            // that candidate is not the global head and this falls through to
            // the normal multi-candidate wait.
            let common_default = candidates[0].prior >= 1.0 && candidates[1].prior < 1.0;
            let dominant = Self::dominance(&candidates[0].value, &predictions);
            if habitual || common_default || dominant {
                candidates.truncate(1);
            }
        }
        let max_candidates = self.config.ui.max_candidates.clamp(1, 50);
        candidates.truncate(if !list {
            max_candidates
        } else if candidates
            .first()
            .is_some_and(|candidate| candidate.source == Source::Filesystem)
        {
            50
        } else {
            // TAB is an explicit "what can go here?", and the answer now
            // leads with subcommands -- so the budget has to leave room for
            // the flags that follow them instead of cutting the list off at
            // the last subcommand.
            (max_candidates * 3).clamp(1, 24)
        });
        Ok(QueryResponse {
            request_id: now_micros(),
            prefix_len: context.raw_current_len,
            candidates,
            cache_only: offline,
            context_key,
        })
    }

    /// True when the top-ranked candidate holds most of the recent learned
    /// probability at this position, according to the next-token/command
    /// model alone (chain completions of the same habit are not counted
    /// twice).
    fn dominance(top_value: &str, predictions: &HashMap<String, f64>) -> bool {
        let Some(top_weight) = predictions.get(top_value) else {
            return false;
        };
        if *top_weight < DOMINANCE_MIN_WEIGHT {
            return false;
        }
        let total: f64 = predictions.values().sum();
        total > 0.0 && *top_weight / total >= DOMINANCE_MIN_SHARE
    }
}

/// The key under which this exact completion position is remembered for
/// accept/reject feedback: the typed command path for arguments, or the
/// prompt-directory plus typed prefix for a command slot.
fn position_key(context: &QueryContext, bare_command: bool, cwd: &Path) -> String {
    if bare_command {
        format!(
            "prompt|{}|{}",
            history::normalize_dir(&cwd.to_string_lossy()),
            context.current
        )
    } else if let Some(command) = context.command.as_deref() {
        exact_path_key(command, &context.args)
    } else {
        String::new()
    }
}

/// The learned-model key for a position: the command plus every argument
/// already typed, exactly as typed (flags included), because `git commit -m`
/// teaches different things than `git commit`.
fn exact_path_key(command: &str, args: &[String]) -> String {
    let mut key = String::from(command);
    for arg in args {
        key.push(' ');
        key.push_str(arg);
    }
    key.chars().take(160).collect()
}

/// The prior-table key for the position being completed: the command plus the
/// subcommands already typed. Values already typed (a branch, a path, a
/// package) make the key unknown, which is the point -- priors then contribute
/// nothing rather than suggesting a subcommand where none belongs.
fn prior_key(context: &QueryContext) -> String {
    let Some(command) = context.command.as_deref() else {
        return String::new();
    };
    let words: Vec<String> = context
        .args
        .iter()
        .filter(|arg| !arg.starts_with('-'))
        .cloned()
        .collect();
    priors::path_key(command, &words)
}
fn add_usage(usage: &mut HashMap<String, Usage>, value: String, recorded: &Usage) {
    let entry = usage.entry(value).or_insert(Usage {
        count: 0,
        last_used: 0,
    });
    entry.count = entry.count.saturating_add(recorded.count);
    entry.last_used = entry.last_used.max(recorded.last_used);
}
fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
fn now_micros() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_micros() as u64
}
