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
    assert_eq!(rebuilt, vec!["<?php", "$a = \"multi", "line\";", "", "  echo $a; // é ✓"]);
}

#[test]
fn line_count_matches_the_file() {
    let highlighter = Highlighter::new();
    assert_eq!(highlighter.highlight(Some(Language::Php), "").len(), 0);
    assert_eq!(highlighter.highlight(Some(Language::Php), "<?php\n").len(), 1);
    assert_eq!(highlighter.highlight(None, "a\nb").len(), 2);
    assert_eq!(highlighter.highlight(None, "a\n\n").len(), 2);
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

#[test]
fn highlights_vue_script_and_style_blocks_in_their_own_languages() {
    let highlighter = Highlighter::new();
    let source = "<template>\n  <div class=\"bin\">{{ label }}</div>\n</template>\n<script setup lang=\"ts\">\nconst count: number = 42;\n</script>\n<style>\n.bin { color: red; }\n</style>\n";
    let lines = highlighter.highlight(Some(Language::Vue), source);
    assert_eq!(lines.len(), 9);
    assert_eq!(class_of(&highlighter, Language::Vue, source, 4, "const"), "keyword");
    assert_eq!(class_of(&highlighter, Language::Vue, source, 4, "42"), "number");
    assert_eq!(class_of(&highlighter, Language::Vue, source, 0, "template"), "tag");
    assert!(lines[7].iter().any(|(class, _)| *class != 0), "CSS highlighted: {:?}", lines[7]);
}

#[test]
fn highlights_twig_tags_filters_strings_and_comments_over_html() {
    let highlighter = Highlighter::new();
    let source = "{% block content %}\n<p class=\"x\">{{ order.number|default('n/a') }}</p>\n{# note #}\n{% endblock %}\n";
    assert_eq!(class_of(&highlighter, Language::Twig, source, 0, "block"), "keyword");
    assert_eq!(class_of(&highlighter, Language::Twig, source, 1, "default"), "function");
    assert_eq!(class_of(&highlighter, Language::Twig, source, 1, "'n/a'"), "string");
    assert_eq!(class_of(&highlighter, Language::Twig, source, 1, "number"), "property");
    assert_eq!(class_of(&highlighter, Language::Twig, source, 2, "{# note #}"), "comment");
    let rebuilt: String = highlighter.highlight(Some(Language::Twig), source).iter().map(|l| l.iter().map(|s| s.1.as_str()).collect::<String>() + "\n").collect();
    assert_eq!(rebuilt, source, "overlay keeps the text intact");
}

#[test]
fn detects_templates_and_styles() {
    assert_eq!(Language::from_path("views/page/index.html.twig"), Some(Language::Twig));
    assert_eq!(Language::from_path("src/component/Bin.vue"), Some(Language::Vue));
    assert_eq!(Language::from_path("app.scss"), Some(Language::Css));
}