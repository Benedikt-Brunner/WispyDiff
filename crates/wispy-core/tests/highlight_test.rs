use wispy_core::highlight::{Highlighter, Language, TOKEN_CLASSES};

fn class_of(highlighter: &Highlighter, lang: Language, source: &str, line: usize, token: &str) -> &'static str {
    let lines = highlighter.highlight(Some(lang), source);
    let (class, _) = lines[line].iter().find(|(_, text)| text.trim() == token).unwrap_or_else(|| {
        panic!("token {token:?} not found as its own segment in {:?}", lines[line])
    });
    TOKEN_CLASSES[*class as usize]
}

#[test]
fn every_builtin_language_query_compiles() {
    // Highlighter::new panics on an invalid query.
    let highlighter = Highlighter::new();
    for lang in Language::ALL {
        let lines = highlighter.highlight(Some(lang), "a\nb");
        assert_eq!(lines.len(), 2, "{lang:?}");
    }
}

#[test]
fn highlights_php_keywords_strings_and_comments() {
    let highlighter = Highlighter::new();
    let source = "<?php\nfunction pick() {\n    return 'bin';\n}\n// done\n";
    assert_eq!(class_of(&highlighter, Language::Php, source, 1, "function"), "keyword");
    assert_eq!(class_of(&highlighter, Language::Php, source, 2, "'bin'"), "string");
    assert_eq!(class_of(&highlighter, Language::Php, source, 4, "// done"), "comment");
}

#[test]
fn highlights_typescript() {
    let highlighter = Highlighter::new();
    let source = "const n: number = 42;\n";
    assert_eq!(class_of(&highlighter, Language::TypeScript, source, 0, "const"), "keyword");
    assert_eq!(class_of(&highlighter, Language::TypeScript, source, 0, "42"), "number");
}

#[test]
fn segments_reassemble_to_the_original_lines() {
    let highlighter = Highlighter::new();
    let source = "<?php\r\n$a = \"multi\nline\";\n\n  echo $a; // é ✓\n";
    let lines = highlighter.highlight(Some(Language::Php), source);
    let rebuilt: Vec<String> = lines.iter().map(|segs| segs.iter().map(|(_, t)| t.as_str()).collect()).collect();
    assert_eq!(rebuilt, vec!["<?php", "$a = \"multi", "line\";", "", "  echo $a; // é ✓", ""]);
}

#[test]
fn unknown_languages_and_huge_lines_fall_back_to_plain_text() {
    let highlighter = Highlighter::new();
    assert_eq!(highlighter.highlight(None, "x = 1"), vec![vec![(0, "x = 1".to_string())]]);

    let minified = format!("var a={};", "1+".repeat(10_000));
    let lines = highlighter.highlight(Some(Language::JavaScript), &minified);
    assert_eq!(lines, vec![vec![(0, minified.clone())]]);
}

#[test]
fn detects_languages_from_paths() {
    assert_eq!(Language::from_path("src/Stock.php"), Some(Language::Php));
    assert_eq!(Language::from_path("a/b.tsx"), Some(Language::Tsx));
    assert_eq!(Language::from_path("phpstan.neon"), Some(Language::Yaml));
    assert_eq!(Language::from_path("Makefile"), None);
    assert_eq!(Language::from_path("a.b/README"), None);
}
