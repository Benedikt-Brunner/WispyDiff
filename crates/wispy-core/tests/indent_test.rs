use wispy_core::indent::indent_unit;

#[test]
fn tab_indented_files_indent_with_a_tab() {
    let go = "func main() {\n\tif ok {\n\t\trun()\n\t}\n}\n";
    assert_eq!(indent_unit(go), "\t");
    let doc_comments = "class A {\n\t/**\n\t * Doc.\n\t */\n\tfoo() {\n\t\tbar();\n\t}\n}\n";
    assert_eq!(indent_unit(doc_comments), "\t");
}

#[test]
fn space_indented_files_indent_by_their_most_common_step() {
    let php = "class A\n{\n    public function f()\n    {\n        return 1;\n    }\n}\n";
    assert_eq!(indent_unit(php), "    ");
    let yaml = "a:\n  b:\n    c: 1\n  d:\n    - e\n";
    assert_eq!(indent_unit(yaml), "  ");
    let continued = "x = f(\n        long,\n        args)\nif x:\n    y()\nif z:\n    w()\n";
    assert_eq!(indent_unit(continued), "    ", "a one-off continuation indent isn't the level");
}

#[test]
fn files_without_indentation_get_four_spaces() {
    assert_eq!(indent_unit("a\nb\n\nc\n"), "    ");
    assert_eq!(indent_unit(""), "    ");
}
