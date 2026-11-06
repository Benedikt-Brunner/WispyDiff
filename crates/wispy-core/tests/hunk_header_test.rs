mod support;

use support::{row_text, OriginRepo};
use wispy_core::highlight::Highlighter;
use wispy_core::highlight_cache::HighlightCache;
use wispy_core::model::row_kind;
use wispy_core::range::{compute_range, RangeSpec};
use wispy_core::repo_store::RepoStore;

const PHP_BEFORE: &str = "<?php
namespace Acme;

class Cleanup
{
    public function __construct(
        private readonly Stock $stock,
    ) {}

    public function cleanUp(array $ids, Context $context): void
    {
        $a = 1;
        $b = 2;
        $c = 3;
        $d = 4;
        $e = 5;
        $f = 6;
    }
}
";

const TS_BEFORE: &str = "export class Picker {
  private pick(x: number): string {
    if (x > 1) {
      const a = 1;
      const b = 2;
      const c = 3;
      const d = 4;
    }
    return '';
  }
}

export const handler = async (e: Event) => {
  const a = 1;
  const b = 2;
  const c = 3;
  const d = 4;
};
";

#[test]
fn hunk_headers_name_the_enclosing_method_not_the_class() {
    let origin = OriginRepo::init();
    origin.write("src/Cleanup.php", PHP_BEFORE);
    origin.write("src/picker.ts", TS_BEFORE);
    origin.commit("base");
    origin.checkout_new("feature");
    origin.write("src/Cleanup.php", &PHP_BEFORE.replace("$e = 5;", "$e = 50;"));
    origin.write("src/picker.ts", &TS_BEFORE.replace("const d = 4;", "const d = 40;"));
    origin.commit("change method bodies");
    origin.publish_pr(7, "feature");
    origin.checkout("main");

    let root = tempfile::tempdir().unwrap();
    let store = RepoStore::new(root.path(), None);
    let git = store.ensure_repo("acme", "shop", &origin.url()).unwrap();
    let fetched = store.fetch_pr(&git, 7, "main").unwrap();
    let spec = RangeSpec { from: fetched.merge_base, heads: vec![fetched.head_sha], prs: vec![0], ignore_whitespace: false };
    let view = compute_range(&git, &spec, &Highlighter::new(), &HighlightCache::default()).unwrap();

    let headers: Vec<String> = view.rows.iter().filter(|r| r.k == row_kind::HUNK).map(row_text).collect();
    assert_eq!(
        headers,
        vec![
            "@@ -13,7 +13,7 @@ public function cleanUp(array $ids, Context $context): void",
            // `if (x > 1) {` is skipped in favour of the method around it.
            "@@ -4,7 +4,7 @@ private pick(x: number): string {",
            "@@ -14,5 +14,5 @@ export const handler = async (e: Event) => {",
        ]
    );
}
