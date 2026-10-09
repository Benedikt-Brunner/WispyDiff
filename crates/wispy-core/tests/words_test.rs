use wispy_core::words::diff_words;

/// The changed text of each side, span by span.
fn changed(old: &str, new: &str) -> Option<(Vec<String>, Vec<String>)> {
    let pick = |line: &str, spans: Vec<(u32, u32)>| {
        let units: Vec<u16> = line.encode_utf16().collect();
        spans.into_iter().map(|(s, e)| String::from_utf16(&units[s as usize..e as usize]).unwrap()).collect()
    };
    let (o, n) = diff_words(old, new)?;
    Some((pick(old, o), pick(new, n)))
}

#[test]
fn marks_only_the_words_that_changed() {
    assert_eq!(
        changed("    return $this->price * $qty;", "    return $this->total * $qty;"),
        Some((vec!["price".into()], vec!["total".into()]))
    );
}

#[test]
fn an_insertion_marks_only_the_new_side() {
    assert_eq!(changed("foo(alpha);", "foo(alpha, b);"), Some((vec![], vec![", b".into()])));
}

#[test]
fn neighbouring_changes_join_across_whitespace() {
    assert_eq!(
        changed("let x = old value here;", "let x = new thing here;"),
        Some((vec!["old value".into()], vec!["new thing".into()]))
    );
}

#[test]
fn rewritten_lines_are_left_unmarked() {
    assert_eq!(changed("$total = array_sum($prices);", "return new Response($body, 404);"), None);
}

#[test]
fn a_pair_is_left_unmarked_when_either_line_is_mostly_new() {
    // Little changed on the old line, but most of the new one is new.
    assert_eq!(changed("sendMail: false,", "return { orderId, warehouseId, sendMail: false };"), None);
}

#[test]
fn identical_lines_are_left_unmarked() {
    assert_eq!(changed("same", "same"), None);
}

#[test]
fn reindented_lines_mark_the_indentation() {
    assert_eq!(changed("  return 1;", "    return 1;"), Some((vec!["  ".into()], vec!["    ".into()])));
}

#[test]
fn offsets_count_utf16_units() {
    let (old, new) = diff_words("€ 😀 price of the item", "€ 😀 total of the item").unwrap();
    assert_eq!(old, vec![(5, 10)], "€ is one unit, 😀 two");
    assert_eq!(new, vec![(5, 10)]);
}
