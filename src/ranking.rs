use crate::{history::Usage, model::Candidate};
use std::{
    collections::HashMap,
    time::{SystemTime, UNIX_EPOCH},
};

pub fn rank(
    mut candidates: Vec<Candidate>,
    prefix: &str,
    usage: &HashMap<String, Usage>,
    fuzzy: bool,
) -> Vec<Candidate> {
    let p = prefix.to_ascii_lowercase();
    let has_prefix_match = !p.is_empty()
        && candidates.iter().any(|candidate| {
            candidate.value.to_ascii_lowercase().starts_with(&p)
                || candidate.display.to_ascii_lowercase().starts_with(&p)
        });
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    for c in &mut candidates {
        let v = c.value.to_ascii_lowercase();
        let d = c.display.to_ascii_lowercase();
        let mut score = c.confidence * 100.0;
        if p.is_empty() {
            score += 5.0
        } else if v == p || d == p {
            score += 55.0
        } else if v.starts_with(&p) || d.starts_with(&p) {
            score += 40.0 - (v.len().min(d.len()).saturating_sub(p.len()) as f64 * 0.02)
        } else if fuzzy {
            let sim = strsim::jaro_winkler(&v, &p).max(strsim::jaro_winkler(&d, &p));
            if sim < 0.55 {
                score -= 100.0
            } else {
                score += sim * 18.0
            }
        } else {
            score -= 100.0
        }
        if let Some(u) = usage.get(&c.value) {
            score += (u.count as f64).ln_1p() * 6.0;
            let age_days = now.saturating_sub(u.last_used) as f64 / 86400.0;
            score += 8.0 / (1.0 + age_days);
        }
        c.score = score;
    }
    candidates.retain(|c| c.score > 0.0);
    if has_prefix_match {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Source;
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
        );
        assert_eq!(r[0].source, Source::Dynamic);
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
        );
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].value, "--dangerously-skip-permissions");
    }
}
