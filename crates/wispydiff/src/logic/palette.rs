//! Pure palette matching and submit choice; callbacks belong to the UI.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Command {
    pub id: String,
    pub label: String,
    pub keyword: Option<String>,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    Open(String),
    Command { id: String, argument: String },
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PaletteMatch {
    pub id: String,
    pub label: String,
    pub action: Action,
}
/// Case-sensitive subsequence match; callers lowercase haystack and needle. Preserves the
/// TS helper's code-point haystack / UTF-16 needle indexing (astral characters do not match).
pub fn fuzzy(haystack: &str, needle: &str) -> bool {
    let wanted: Vec<u16> = needle.encode_utf16().collect();
    let mut i = 0;
    for ch in haystack.chars() {
        if ch.len_utf16() == 1 && wanted.get(i).copied() == Some(ch as u16) {
            i += 1;
        }
    }
    i == wanted.len()
}
/// Same permissive recognizer as the React palette; validation remains the core's job.
pub fn looks_like_pr(s: &str) -> bool {
    let trimmed = s.trim_end_matches(super::js_whitespace);
    let hash = trimmed
        .rsplit_once('#')
        .is_some_and(|(_, n)| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()));
    hash || s
        .match_indices("/pull/")
        .any(|(i, _)| s.as_bytes().get(i + 6).is_some_and(u8::is_ascii_digit))
}
pub fn matches(query: &str, commands: &[Command], recents: &[String]) -> Vec<PaletteMatch> {
    if let Some(rest) = query.strip_prefix('>') {
        let rest = super::js_trim(rest);
        let q = rest.to_lowercase();
        commands
            .iter()
            .filter_map(|c| {
                let keyword = c
                    .keyword
                    .as_deref()
                    .filter(|k| !k.is_empty() && q.starts_with(&format!("{k} ")));
                if keyword.is_none() && !fuzzy(&c.label.to_lowercase(), &q) {
                    return None;
                }
                let argument = keyword.map_or_else(String::new, |k| {
                    String::from_utf16_lossy(
                        &rest
                            .encode_utf16()
                            .skip(k.encode_utf16().count() + 1)
                            .collect::<Vec<_>>(),
                    )
                });
                let label = if argument.is_empty() {
                    c.label.clone()
                } else {
                    format!("{} “{}”", c.label, argument)
                };
                Some(PaletteMatch {
                    id: c.id.clone(),
                    label,
                    action: Action::Command {
                        id: c.id.clone(),
                        argument,
                    },
                })
            })
            .take(10)
            .collect()
    } else {
        let q = super::js_trim(query).to_lowercase();
        recents
            .iter()
            .filter(|r| q.is_empty() || fuzzy(&r.to_lowercase(), &q))
            .take(8)
            .map(|r| PaletteMatch {
                id: r.clone(),
                label: r.clone(),
                action: Action::Open(r.clone()),
            })
            .collect()
    }
}
/// A PR-shaped input wins over the selected recent. Otherwise submit the selected result,
/// or a nonempty raw PR input when there are no matches. Commands never fall back to opening.
pub fn submit(query: &str, selected: usize, matches: &[PaletteMatch]) -> Option<Action> {
    let command = query.starts_with('>');
    let q = super::js_trim(query);
    if !command && looks_like_pr(query) {
        return Some(Action::Open(q.into()));
    }
    matches
        .get(selected)
        .map(|m| m.action.clone())
        .or_else(|| (!command && !q.is_empty()).then(|| Action::Open(q.into())))
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn commands_keyword_argument_and_limits() {
        let c = Command {
            id: "ignore".into(),
            label: "Ignore files".into(),
            keyword: Some("ignore".into()),
        };
        let m = matches("> IGNORE src/Foo/**  ", &[c.clone()], &[]);
        assert_eq!(
            m[0].action,
            Action::Command {
                id: "ignore".into(),
                argument: "src/Foo/**".into()
            }
        );
        assert_eq!(m[0].label, "Ignore files “src/Foo/**”");
        assert_eq!(matches(">igf", &[c.clone()], &[]).len(), 1);
        assert!(matches(" >ignore", &[c.clone()], &[]).is_empty());
        assert!(matches(">ignore\tx", &[c.clone()], &[]).is_empty());
        assert_eq!(matches(">", &vec![c; 12], &[]).len(), 10);
    }
    #[test]
    fn recent_matching_and_submission() {
        let recent = vec!["a/repo#1".into(), "b/repo#2".into()];
        let m = matches("rp2", &[], &recent);
        assert_eq!(m.len(), 1);
        assert_eq!(submit("rp2", 0, &m), Some(Action::Open("b/repo#2".into())));
        assert_eq!(
            submit(" x/y#42 ", 0, &m),
            Some(Action::Open("x/y#42".into()))
        );
        assert_eq!(
            submit("anything", 0, &[]),
            Some(Action::Open("anything".into()))
        );
        assert!(submit(">unknown", 0, &[]).is_none());
        assert!(submit(" ", 0, &[]).is_none());
        assert!(looks_like_pr("https://github.com/a/b/pull/12/files"));
        assert!(!looks_like_pr("a/b#x"));
    }
    #[test]
    fn subsequences_preserve_spaces_case_and_unicode_semantics() {
        assert!(fuzzy("abc", "ac"));
        assert!(!fuzzy("abc", "ca"));
        assert!(fuzzy("anything", ""));
        assert!(!fuzzy("ABC", "ab"));
        assert!(!fuzzy("abc", "a c"));
        assert!(!fuzzy("😀", "😀"));
        assert!(fuzzy("éclair", "él"));
        let c = Command {
            id: "unicode".into(),
            label: "Command".into(),
            keyword: Some("i\u{307}".into()),
        };
        let m = matches(">İ foo", &[c], &[]);
        assert_eq!(
            m[0].action,
            Action::Command {
                id: "unicode".into(),
                argument: "oo".into()
            }
        );
    }
}
