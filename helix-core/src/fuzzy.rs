use std::cmp::Reverse;
use std::ops::DerefMut;

use nucleo::pattern::{Atom, AtomKind, CaseMatching, Normalization};
use nucleo::{Config, Utf32Str};
use parking_lot::Mutex;

pub struct LazyMutex<T> {
    inner: Mutex<Option<T>>,
    init: fn() -> T,
}

impl<T> LazyMutex<T> {
    pub const fn new(init: fn() -> T) -> Self {
        Self {
            inner: Mutex::new(None),
            init,
        }
    }

    pub fn lock(&self) -> impl DerefMut<Target = T> + '_ {
        parking_lot::MutexGuard::map(self.inner.lock(), |val| val.get_or_insert_with(self.init))
    }
}

pub static MATCHER: LazyMutex<nucleo::Matcher> = LazyMutex::new(nucleo::Matcher::default);

/// convenience function to easily fuzzy match
/// on a (relatively small list of inputs). This is not recommended for building a full tui
/// application that can match large numbers of matches as all matching is done on the current
/// thread, effectively blocking the UI
pub fn fuzzy_match<T: AsRef<str>>(
    pattern: &str,
    items: impl IntoIterator<Item = T>,
    path: bool,
) -> Vec<(T, u16)> {
    let mut matcher = MATCHER.lock();
    matcher.config = Config::DEFAULT;
    if path {
        matcher.config.set_match_paths();
    }
    let pattern = Atom::new(
        pattern,
        CaseMatching::Smart,
        Normalization::Smart,
        AtomKind::Fuzzy,
        false,
    );
    pattern.match_list(items, &mut matcher)
}

/// Like [`fuzzy_match`], but also keeps items a small typo away from `pattern`.
///
/// Nucleo subsequence matches always rank first. When the pattern is at least
/// three characters, unmatched items are compared with a bounded
/// Damerau-Levenshtein distance against the same-length prefix and against
/// hyphen-separated segments so names like `config-open` still surface for
/// `congig`.
pub fn fuzzy_match_with_typos<T: AsRef<str>>(
    pattern: &str,
    items: impl IntoIterator<Item = T>,
) -> Vec<(T, u16)> {
    if pattern.is_empty() {
        return items.into_iter().map(|item| (item, 0)).collect();
    }

    let mut matcher = MATCHER.lock();
    matcher.config = Config::DEFAULT;
    let atom = Atom::new(
        pattern,
        CaseMatching::Smart,
        Normalization::Smart,
        AtomKind::Fuzzy,
        false,
    );

    let max_distance = max_typo_distance(pattern);
    let ignore_case = pattern.chars().all(|c| !c.is_uppercase());
    let mut buf = Vec::new();
    let mut fuzzy = Vec::new();
    let mut typos = Vec::new();

    for item in items {
        let text = item.as_ref();
        if let Some(score) = atom.score(Utf32Str::new(text, &mut buf), &mut matcher) {
            fuzzy.push((item, score));
        } else if max_distance > 0 {
            if let Some(distance) = typo_distance(pattern, text, max_distance, ignore_case) {
                typos.push((item, distance));
            }
        }
    }

    fuzzy.sort_by_key(|(_, score)| Reverse(*score));
    typos.sort_by(|a, b| {
        a.1.cmp(&b.1)
            .then_with(|| a.0.as_ref().len().cmp(&b.0.as_ref().len()))
            .then_with(|| a.0.as_ref().cmp(b.0.as_ref()))
    });
    fuzzy.extend(
        typos
            .into_iter()
            .map(|(item, distance)| (item, u16::from(max_distance.saturating_sub(distance)))),
    );
    fuzzy
}

fn max_typo_distance(pattern: &str) -> u8 {
    match pattern.chars().count() {
        0..=2 => 0,
        3..=4 => 1,
        _ => 2,
    }
}

fn fold_chars(s: &str, ignore_case: bool) -> Vec<char> {
    if ignore_case {
        s.chars().flat_map(char::to_lowercase).collect()
    } else {
        s.chars().collect()
    }
}

fn typo_distance(pattern: &str, candidate: &str, max: u8, ignore_case: bool) -> Option<u8> {
    let query = fold_chars(pattern, ignore_case);
    let mut best = None;

    let mut consider = |text: &str| {
        let candidate = fold_chars(text, ignore_case);
        if candidate.is_empty() {
            return;
        }
        let haystack = if query.len() > candidate.len() {
            candidate.as_slice()
        } else {
            &candidate[..query.len()]
        };
        if let Some(distance) = bounded_damerau_levenshtein(&query, haystack, max as usize) {
            best = Some(best.map_or(distance, |best: u8| best.min(distance)));
        }
    };

    consider(candidate);
    for segment in candidate.split('-') {
        if segment != candidate {
            consider(segment);
        }
    }

    best
}

/// Optimal string alignment: Levenshtein plus adjacent transpositions.
fn bounded_damerau_levenshtein(a: &[char], b: &[char], max: usize) -> Option<u8> {
    let n = a.len();
    let m = b.len();
    if n.abs_diff(m) > max {
        return None;
    }
    if n == 0 {
        return u8::try_from(m)
            .ok()
            .filter(|&distance| (distance as usize) <= max);
    }

    let mut prev_prev = vec![0usize; m + 1];
    let mut prev: Vec<usize> = (0..=m).collect();
    let mut curr = vec![0usize; m + 1];

    for i in 1..=n {
        curr[0] = i;
        let mut row_min = curr[0];
        for j in 1..=m {
            let cost = usize::from(a[i - 1] != b[j - 1]);
            let mut distance = (prev[j] + 1).min(curr[j - 1] + 1).min(prev[j - 1] + cost);
            if i > 1 && j > 1 && a[i - 1] == b[j - 2] && a[i - 2] == b[j - 1] {
                distance = distance.min(prev_prev[j - 2] + 1);
            }
            curr[j] = distance;
            row_min = row_min.min(distance);
        }
        if row_min > max {
            return None;
        }
        std::mem::swap(&mut prev_prev, &mut prev);
        std::mem::swap(&mut prev, &mut curr);
    }

    let distance = prev[m];
    if distance <= max {
        u8::try_from(distance).ok()
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const COMMANDS: &[&str] = &[
        "config-reload",
        "config-open",
        "config-open-workspace",
        "write",
        "write-quit",
        "quit",
        "open",
        "theme",
        "set-option",
        "change-current-directory",
    ];

    fn names(pattern: &str) -> Vec<&'static str> {
        fuzzy_match_with_typos(pattern, COMMANDS.iter().copied())
            .into_iter()
            .map(|(name, _)| name)
            .collect()
    }

    #[test]
    fn empty_query_returns_all() {
        assert_eq!(names("").len(), COMMANDS.len());
    }

    #[test]
    fn subsequence_still_matches_config_commands() {
        let matched = names("cfg");
        assert!(matched.contains(&"config-reload"));
        assert!(matched.contains(&"config-open"));
        assert!(matched.contains(&"config-open-workspace"));
    }

    #[test]
    fn substitution_typo_keeps_config_commands() {
        let matched = names("congig");
        assert!(
            matched.iter().any(|name| name.starts_with("config")),
            "{matched:?}"
        );
        assert!(matched.contains(&"config-open"));
        assert!(matched.contains(&"config-reload"));
        assert!(!matched.contains(&"change-current-directory"));
    }

    #[test]
    fn transposition_typo_ranks_write_first() {
        let matched = names("wirte");
        assert_eq!(matched[0], "write");
        assert!(matched.contains(&"write-quit"));
    }

    #[test]
    fn extra_letter_typo_keeps_write() {
        let matched = names("writee");
        assert_eq!(matched[0], "write");
    }

    #[test]
    fn short_query_does_not_typo_match_everything() {
        assert!(names("z").is_empty());
        assert!(names("zz").is_empty());
    }

    #[test]
    fn garbage_does_not_dump_full_list() {
        assert!(names("zzzzzz").is_empty());
    }

    #[test]
    fn hyphen_segment_typo_matches_second_word() {
        let matched = names("relaod");
        assert!(matched.contains(&"config-reload"), "{matched:?}");
    }
}
