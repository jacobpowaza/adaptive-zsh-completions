//! Ranking answers "given everything typed so far, what is the user most
//! likely to type next?" -- not "which strings start with what they typed".
//!
//! Every candidate is scored as a sum of independent, separately tunable
//! terms, in the order they matter:
//!
//! 1. validity at this position: source confidence (set by whoever produced
//!    the candidate) and kind -- a subcommand slot prefers subcommands to
//!    flags
//! 2. global/common usage of this token here (`crate::priors`)
//! 3. this user's frequency at this position
//! 4. this user's recency at this position
//! 5. how well it matches the typed prefix, when there is one
//!
//! Nothing here is command-specific: `git`, `brew` and an unknown in-house
//! tool all travel the same path, they just arrive carrying different priors.

use crate::{
    config::FuzzyPreset,
    history::Usage,
    model::{Candidate, CandidateKind},
};
use std::{
    collections::HashMap,
    time::{SystemTime, UNIX_EPOCH},
};

/// Scale of the source-confidence term. See its use below.
const TRUST_WEIGHT: f64 = 40.0;
/// How much a top-of-list global default is worth. Large enough to decide an
/// otherwise flat field (an untyped `brew `), small enough that a typed prefix
/// or a personal habit overrules it.
const PRIOR_WEIGHT: f64 = 24.0;
/// How far a flag drops when the cursor is not in a flag position. Enough to
/// put every subcommand above every flag, not enough to hide flags entirely --
/// they still follow in the TAB list.
const FLAG_PENALTY: f64 = 14.0;
/// Explicit rejection signal. Must be able to bury a dismissed candidate
/// below rivals that were never dismissed, without deleting it entirely.
const REJECT_PENALTY: f64 = 26.0;

pub fn rank(
    candidates: Vec<Candidate>,
    prefix: &str,
    usage: &HashMap<String, Usage>,
    fuzzy: bool,
    preset: FuzzyPreset,
    rejected: &HashMap<String, Usage>,
) -> Vec<Candidate> {
    rank_inner(candidates, prefix, usage, fuzzy, preset, false, rejected)
}

pub fn rank_for_list(
    mut candidates: Vec<Candidate>,
    prefix: &str,
    usage: &HashMap<String, Usage>,
    fuzzy: bool,
    preset: FuzzyPreset,
    rejected: &HashMap<String, Usage>,
) -> Vec<Candidate> {
    let p = prefix.to_ascii_lowercase();
    let group_prefix = if p.starts_with('-') {
        p.clone()
    } else {
        p.chars().next().map(String::from).unwrap_or_default()
    };
    if !group_prefix.is_empty() {
        candidates.retain(|candidate| {
            candidate
                .value
                .to_ascii_lowercase()
                .starts_with(&group_prefix)
                || candidate
                    .display
                    .to_ascii_lowercase()
                    .starts_with(&group_prefix)
        });
    }
    rank_inner(candidates, prefix, usage, fuzzy, preset, true, rejected)
}

fn rank_inner(
    candidates: Vec<Candidate>,
    prefix: &str,
    usage: &HashMap<String, Usage>,
    fuzzy: bool,
    preset: FuzzyPreset,
    include_alternatives: bool,
    rejected: &HashMap<String, Usage>,
) -> Vec<Candidate> {
    let p = prefix.to_ascii_lowercase();
    let prefix_chars: Vec<char> = p.chars().collect();
    let has_prefix_match = !p.is_empty()
        && candidates.iter().any(|candidate| {
            candidate.value.to_ascii_lowercase().starts_with(&p)
                || candidate.display.to_ascii_lowercase().starts_with(&p)
        });
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    // A leading "-" is the user explicitly asking for options; anywhere else,
    // a flag is the least likely thing they meant, however well it matches.
    let flag_position = p.starts_with('-');
    // `weak` marks candidates the user's typed prefix rules out entirely
    // (fuzzy misses, non-matches). They vanish in every mode; a merely
    // low-scoring candidate -- e.g. one the user rejected -- only vanishes
    // from the inline ghost, never from the explicit TAB list.
    let mut scored: Vec<(Candidate, bool)> = Vec::with_capacity(candidates.len());
    for c in candidates {
        let v = c.value.to_ascii_lowercase();
        let d = c.display.to_ascii_lowercase();
        // Source trust: how sure we are this token is even valid here. Scaled
        // so the whole trust range (~22..39) is comparable to one prior step
        // rather than swamping it -- a tool's own help text being the source
        // must not outrank "this is what people actually run".
        let mut score = c.confidence * TRUST_WEIGHT;
        // Global usage. Weighted so a top-of-list default outranks anything
        // merely valid, and so a couple of real personal uses (below) can
        // still overtake it.
        score += c.prior * PRIOR_WEIGHT;
        if c.kind == CandidateKind::Flag && !flag_position {
            score -= FLAG_PENALTY;
        }
        let mut weak = false;
        if p.is_empty() {
            score += 5.0
        } else if v == p || d == p {
            score += 55.0
        } else if v.starts_with(&p) || d.starts_with(&p) {
            score += 40.0 - (v.len().min(d.len()).saturating_sub(p.len()) as f64 * 0.02)
        } else if fuzzy {
            // Fuzzy matching is a bounded subsequence test (the preset says
            // how far typed letters may sprawl through the candidate) plus a
            // Jaro-Winkler quality bar. `gco` reaches `git checkout` under
            // the default smart preset but cannot wander into
            // `git remote add origin`.
            let cand_chars: Vec<char> = v.chars().collect();
            let disp_chars: Vec<char> = d.chars().collect();
            let sprawl = bounded_subsequence(&cand_chars, &prefix_chars, preset.max_gap())
                .or_else(|| bounded_subsequence(&disp_chars, &prefix_chars, preset.max_gap()));
            match sprawl {
                Some(compactness) => {
                    let sim = strsim::jaro_winkler(&v, &p).max(strsim::jaro_winkler(&d, &p));
                    if sim < 0.55 {
                        if !include_alternatives {
                            weak = true;
                        }
                    } else {
                        score += sim * 18.0 + compactness;
                    }
                }
                None => {
                    if !include_alternatives {
                        weak = true;
                    }
                }
            }
        } else if !include_alternatives {
            weak = true;
        }
        // Personal frequency and recency. Deliberately able to out-score the
        // prior term: a handful of real uses is better evidence about this
        // user than any global default.
        if let Some(u) = usage.get(&c.value) {
            score += (u.count as f64).ln_1p() * 9.0;
            let age_days = now.saturating_sub(u.last_used) as f64 / 86400.0;
            score += 10.0 / (1.0 + age_days);
        }
        // Explicit rejection feedback at this exact position: pushes the
        // candidate below untouched rivals without deleting it from the
        // list. Acceptance is folded into `usage` upstream, where it simply
        // strengthens frequency and recency like another real use would.
        if let Some(u) = rejected.get(&c.value) {
            score -= (u.count as f64).ln_1p() * REJECT_PENALTY;
            let age_days = now.saturating_sub(u.last_used) as f64 / 86400.0;
            score -= 8.0 / (1.0 + age_days);
        }
        scored.push((Candidate { score, ..c }, weak));
    }
    scored.retain(|(c, weak)| !weak && (c.score > 0.0 || include_alternatives));
    let mut candidates: Vec<Candidate> = scored.into_iter().map(|(c, _)| c).collect();
    if has_prefix_match && !include_alternatives {
        candidates.retain(|candidate| {
            candidate.value.to_ascii_lowercase().starts_with(&p)
                || candidate.display.to_ascii_lowercase().starts_with(&p)
        });
    }
    candidates.sort_by(|a, b| {
        b.score
            .total_cmp(&a.score)
            .then_with(|| a.value.cmp(&b.value))
    });
    candidates.dedup_by(|a, b| a.value == b.value);
    candidates
}

/// Matches `prefix` against `candidate` as an in-order subsequence in which
/// at most `max_gap` unmatched characters may sit between two consecutive
/// typed characters. Returns a compactness bonus (roughly 0..=8) that
/// rewards runs of adjacent characters and matches anchored at a word
/// boundary, or `None` when every anchoring attempt violates the gap bound.
///
/// Backtracking is limited to re-anchoring on earlier occurrences of the
/// first typed character; pathological middle-of-candidate ambiguities are
/// accepted as misses, which only ever *hides* a suggestion.
fn bounded_subsequence(candidate: &[char], prefix: &[char], max_gap: usize) -> Option<f64> {
    if prefix.is_empty() {
        return Some(0.0);
    }
    let lower = |c: char| c.to_ascii_lowercase();
    for start in 0..candidate.len() {
        let Some(anchor) =
            (start..candidate.len()).find(|&i| lower(candidate[i]) == lower(prefix[0]))
        else {
            break;
        };
        let mut last = anchor;
        let mut matched = 1usize;
        let mut consecutive = 0u32;
        let mut skipped_total = 0usize;
        let mut viable = true;
        for &pc in &prefix[1..] {
            let Some(pos) =
                ((last + 1)..candidate.len()).find(|&i| lower(candidate[i]) == lower(pc))
            else {
                viable = false;
                break;
            };
            let skipped = pos - last - 1;
            if skipped > max_gap {
                viable = false;
                break;
            }
            if skipped == 0 {
                consecutive += 1;
            }
            skipped_total += skipped;
            last = pos;
            matched += 1;
        }
        if viable && matched == prefix.len() {
            let span = last - anchor + 1;
            let mut bonus = (consecutive.min(6) as f64)
                + (1.0 - (skipped_total as f64 / span.max(1) as f64).min(1.0)) * 2.0;
            let boundary = anchor == 0 || !candidate[anchor - 1].is_ascii_alphanumeric();
            if boundary {
                bonus += 2.0;
            }
            return Some(bonus);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Source;

    fn empty_rejected() -> HashMap<String, Usage> {
        HashMap::new()
    }
    fn smart() -> FuzzyPreset {
        FuzzyPreset::Smart
    }
    #[test]
    fn exact_prefix_beats_fuzzy() {
        let r = rank(
            vec![
                Candidate::new("--verbose", "", Source::LocalHelp),
                Candidate::new("--version", "", Source::LocalHelp),
            ],
            "--verb",
            &HashMap::new(),
            true,
            smart(),
            &empty_rejected(),
        );
        assert_eq!(r[0].value, "--verbose");
    }
    #[test]
    fn stronger_source_wins_equal_match() {
        let r = rank(
            vec![
                Candidate::new("dev", "", Source::History),
                Candidate::new("dev", "", Source::Dynamic),
            ],
            "d",
            &HashMap::new(),
            true,
            smart(),
            &empty_rejected(),
        );
        assert_eq!(r[0].source, Source::Dynamic);
    }
    #[test]
    fn subcommands_outrank_flags_when_no_dash_was_typed() {
        let mut flag = Candidate::new("--debug", "", Source::LocalHelp);
        flag.prior = 0.0;
        let sub =
            Candidate::new("install", "", Source::LocalHelp).with_kind(CandidateKind::Subcommand);
        let ranked = rank_for_list(
            vec![flag, sub],
            "",
            &HashMap::new(),
            true,
            smart(),
            &empty_rejected(),
        );
        assert_eq!(ranked[0].value, "install");
        assert_eq!(
            ranked[1].value, "--debug",
            "flags stay available, just last"
        );
        // ...and asking for a flag still gets flags.
        let ranked = rank_for_list(
            vec![
                Candidate::new("--debug", "", Source::LocalHelp),
                Candidate::new("install", "", Source::LocalHelp)
                    .with_kind(CandidateKind::Subcommand),
            ],
            "--",
            &HashMap::new(),
            true,
            smart(),
            &empty_rejected(),
        );
        assert_eq!(ranked[0].value, "--debug");
    }

    #[test]
    fn priors_order_an_untyped_position() {
        let mut common = Candidate::new("install", "", Source::LocalHelp);
        common.prior = 1.0;
        let mut obscure = Candidate::new("analytics", "", Source::LocalHelp);
        obscure.prior = 0.0;
        let ranked = rank(
            vec![obscure, common],
            "",
            &HashMap::new(),
            true,
            smart(),
            &empty_rejected(),
        );
        assert_eq!(ranked[0].value, "install");
    }

    #[test]
    fn personal_history_overrides_the_global_default() {
        let mut common = Candidate::new("install", "", Source::LocalHelp);
        common.prior = 1.0;
        let mut habit = Candidate::new("update", "", Source::LocalHelp);
        habit.prior = 0.6;
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let usage = HashMap::from([(
            "update".to_owned(),
            Usage {
                count: 6,
                last_used: now,
            },
        )]);
        let ranked = rank(
            vec![common, habit],
            "",
            &usage,
            true,
            smart(),
            &empty_rejected(),
        );
        assert_eq!(ranked[0].value, "update");
    }

    #[test]
    fn prefix_match_suppresses_weak_fuzzy_noise() {
        let r = rank(
            vec![
                Candidate::new("--dangerously-skip-permissions", "", Source::LocalHelp),
                Candidate::new("--agents", "", Source::LocalHelp),
            ],
            "--dang",
            &HashMap::new(),
            true,
            smart(),
            &empty_rejected(),
        );
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].value, "--dangerously-skip-permissions");
    }

    #[test]
    fn rejection_demotes_but_keeps_a_candidate() {
        let a = Candidate::new("alpha", "", Source::History);
        let b = Candidate::new("beta", "", Source::History);
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let rejected = HashMap::from([(
            "beta".to_owned(),
            Usage {
                count: 2,
                last_used: now,
            },
        )]);
        let ranked = rank(
            vec![a.clone(), b.clone()],
            "",
            &HashMap::new(),
            true,
            smart(),
            &rejected,
        );
        assert_eq!(ranked[0].value, "alpha", "dismissed beta drops below alpha");
        let listed = rank_for_list(vec![a, b], "", &HashMap::new(), true, smart(), &rejected);
        assert_eq!(listed.len(), 2, "rejection hides nothing from the TAB list");
    }

    #[test]
    fn fuzzy_presets_bound_how_far_typed_letters_sprawl() {
        // One candidate per rung of the ladder: `gl` with the `l` sitting
        // adjacent, four characters away, and seven characters away.
        let adjacent = Candidate::new("glossary", "", Source::History);
        let mid = Candidate::new("git clone url", "", Source::History);
        // Four spaces put the `l` seven characters past the `g`: inside
        // loose's bound of 8, outside smart's 4.
        let far = Candidate::new("git    clone", "", Source::History);
        // The explicit TAB list is where bounded-fuzzy alternatives surface;
        // inline ghost mode additionally requires shared typed prefixes.
        // Unmatched candidates stay listed but receive no fuzzy credit
        // (their raw base score is untouched), so the preset's effect shows
        // up as score deltas. Note the two gates are independent by design:
        // the sprawl bound decides *candidacy* (bounded_subsequence), while
        // Jaro-Winkler decides *quality* -- a legal-but-sprawling match in a
        // long candidate can still fail quality, which only ever hides a
        // ghost, never fabricates one.
        let scored = |preset: FuzzyPreset| {
            rank_for_list(
                vec![adjacent.clone(), mid.clone(), far.clone()],
                "gl",
                &HashMap::new(),
                true,
                preset,
                &empty_rejected(),
            )
            .into_iter()
            .map(|c| (c.value, c.score))
            .collect::<Vec<_>>()
        };
        let base = Source::History.base_confidence() * TRUST_WEIGHT;
        let credit = |rows: &[(String, f64)], value: &str| {
            rows.iter()
                .find(|(v, _)| v == value)
                .map(|(_, s)| *s - base)
                .unwrap_or(f64::NEG_INFINITY)
        };
        let tight = scored(FuzzyPreset::Tight);
        assert!(credit(&tight, "glossary") > 5.0, "adjacent rung matches");
        assert!(
            credit(&tight, "git clone url") < 5.0 && credit(&tight, "git    clone") < 5.0,
            "tight grants nothing to gaps beyond 1"
        );
        let smart_rows = scored(FuzzyPreset::Smart);
        assert!(
            credit(&smart_rows, "git clone url") > 5.0,
            "gap 4 fits smart"
        );
        assert!(
            credit(&smart_rows, "git    clone") < 5.0,
            "gap 7 exceeds smart"
        );
        // Prefix-exact hits never enter the fuzzy branch at all.
        let r = rank(
            vec![Candidate::new("gl", "", Source::History)],
            "gl",
            &HashMap::new(),
            true,
            FuzzyPreset::Tight,
            &empty_rejected(),
        );
        assert_eq!(r.len(), 1);
    }

    #[test]
    fn bounded_subsequence_prefers_compact_and_boundary_matches() {
        let chars: Vec<char> = "git checkout".chars().collect();
        let gco: Vec<char> = "gco".chars().collect();
        let bonus = bounded_subsequence(&chars, &gco, 4).expect("smart matches gco");
        assert!(bonus > 0.0);
        // No match once the bound forbids the gaps entirely.
        assert!(bounded_subsequence(&chars, &gco, 0).is_none());
        // Prefix longer than any in-order path is a miss.
        let long: Vec<char> = "zzzz".chars().collect();
        assert!(bounded_subsequence(&chars, &long, 8).is_none());
        // Empty prefix trivially matches.
        assert_eq!(bounded_subsequence(&chars, &[], 4), Some(0.0));
        // The loose bound accepts gaps that tight and smart must refuse:
        // `l` sits seven characters past `g` here.
        let far: Vec<char> = "git    clone".chars().collect();
        let gl: Vec<char> = "gl".chars().collect();
        assert!(bounded_subsequence(&far, &gl, 1).is_none());
        assert!(bounded_subsequence(&far, &gl, 4).is_none());
        assert!(bounded_subsequence(&far, &gl, 7).is_some());
        assert!(bounded_subsequence(&far, &gl, 8).is_some());
    }
}
