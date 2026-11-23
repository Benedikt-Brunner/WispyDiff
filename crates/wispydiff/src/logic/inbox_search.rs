//! The inbox's type-to-find (`f`): PRs matched by number, title or branch name.

use super::fuzzy::match_path;

/// How well `query` matches a PR (higher is better); none when it doesn't. Every word of the
/// query must match the PR's number (from its start, `#` optional), or fuzzily its title or
/// its head branch; the best of those counts for the word.
pub fn score(query: &str, number: u64, title: &str, branch: &str) -> Option<f64> {
    let number = number.to_string();
    let mut total = 0.;
    let mut words = 0;
    for word in query.split_whitespace() {
        words += 1;
        let digits = word.strip_prefix('#').unwrap_or(word);
        let by_number = (!digits.is_empty() && number.starts_with(digits)).then(|| {
            // A whole number beats any text match, a longer prefix a shorter one.
            if digits == number { 1000. } else { 500. + digits.len() as f64 }
        });
        // Fuzzy (word starts rank high), the word as it is in the text higher still, and
        // highest at the text's start.
        let lower = word.to_lowercase();
        let text_score = |text: &str| {
            let fuzzy = match_path(text, word)?.score;
            let text = text.to_lowercase();
            let whole = if text.contains(&lower) { 5. * lower.chars().count() as f64 } else { 0. };
            let first = if text.starts_with(&lower) { 5. } else { 0. };
            Some(fuzzy + whole + first)
        };
        let by_text = [title, branch].into_iter().filter_map(text_score).reduce(f64::max);
        total += [by_number, by_text].into_iter().flatten().reduce(f64::max)?;
    }
    (words > 0).then_some(total)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_number_title_and_branch() {
        assert!(score("12552", 12552, "Fix login", "tim/fix-login").is_some());
        assert!(score("#125", 12552, "Fix login", "tim/fix-login").is_some());
        assert!(score("552", 12552, "Fix login", "tim/fix-login").is_none());
        assert!(score("login", 1, "Fix login", "main").is_some());
        assert!(score("tim/fx", 1, "Something", "tim/fix-login").is_some());
        // Fuzzy: the letters in order.
        assert!(score("fxlgn", 1, "Fix login", "main").is_some());
        assert!(score("nigol", 1, "Fix login", "main").is_none());
        // Every word must match somewhere.
        assert!(score("fix 1", 12, "Fix login", "main").is_some());
        assert!(score("fix 3", 12, "Fix login", "main").is_none());
        assert!(score("  ", 12, "Fix login", "main").is_none());
    }

    #[test]
    fn ranks_the_exact_number_and_closer_text_first() {
        let exact = score("12", 12, "Other", "x").unwrap();
        let prefix = score("12", 125, "Other", "x").unwrap();
        assert!(exact > prefix);
        let first = score("capacity", 4, "Capacity docs and tests", "x").unwrap();
        assert!(first > score("capacity", 3, "Show capacity in admin", "x").unwrap());
        let tight = score("login", 1, "Fix login page", "x").unwrap();
        let loose = score("login", 2, "Load other good items now", "x").unwrap();
        assert!(tight > loose);
    }
}
