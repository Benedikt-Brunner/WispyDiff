use wispy_core::pr_ref::PrRef;

fn pr(owner: &str, repo: &str, number: u64) -> PrRef {
    PrRef { owner: owner.into(), repo: repo.into(), number }
}

#[test]
fn parses_urls_and_short_forms() {
    let expected = pr("acme", "widgets", 812);
    for input in [
        "https://github.com/acme/widgets/pull/812",
        "https://github.com/acme/widgets/pull/812/files",
        "https://github.com/acme/widgets/pull/812/files#diff-abc",
        "https://github.com/acme/widgets/pull/812?w=1",
        "http://www.github.com/acme/widgets/pull/812/",
        "github.com/acme/widgets/pull/812",
        "acme/widgets/pull/812",
        "acme/widgets#812",
        "  acme/widgets#812\n",
    ] {
        assert_eq!(PrRef::parse(input).unwrap(), expected, "input: {input:?}");
    }
}

#[test]
fn accepts_dots_and_underscores_in_names() {
    assert_eq!(PrRef::parse("my_org/repo.js#3").unwrap(), pr("my_org", "repo.js", 3));
}

#[test]
fn rejects_non_pr_input() {
    for input in [
        "",
        "acme",
        "acme/widgets",
        "acme/widgets#",
        "acme/widgets#abc",
        "acme/widgets#0",
        "https://github.com/acme/widgets/issues/812",
        "a/b/c#1",
        "../etc#1",
        "owner/re po#1",
    ] {
        assert!(PrRef::parse(input).is_err(), "should reject {input:?}");
    }
}

#[test]
fn displays_as_short_form() {
    assert_eq!(pr("a", "b", 7).to_string(), "a/b#7");
}
