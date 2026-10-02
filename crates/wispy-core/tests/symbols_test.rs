mod support;

use support::{pull_request, OriginRepo};
use wispy_core::anchors::locate_new_line;
use wispy_core::cache::Cache;
use wispy_core::github::GitHubClient;
use wispy_core::highlight::Highlighter;
use wispy_core::repo_store::RepoStore;
use wispy_core::service::PrService;
use wispy_core::stack::{Stack, StackSnapshot};

const PHP_BEFORE: &str = "<?php\nfinal class Allocator\n{\n    public function allocate(int $qty): int\n    {\n        return $qty;\n    }\n}\n";
const PHP_AFTER: &str = "<?php\nfinal class Allocator\n{\n    public function allocate(int $qty): int\n    {\n        return max($qty, 1);\n    }\n}\n";

fn setup() -> (OriginRepo, PrService, StackSnapshot, tempfile::TempDir) {
    let origin = OriginRepo::init();
    origin.write("src/Allocator.php", PHP_BEFORE);
    origin.write("src/Picker.ts", "import { allocate } from './api';\n\nexport function pick(qty: number) {\n  return allocate(qty);\n}\n");
    origin.write("src/Unrelated.php", "<?php\n// mentions allocate only here\n");
    origin.commit("base");
    origin.checkout_new("feature");
    origin.write("src/Allocator.php", PHP_AFTER);
    origin.write("src/Picker.ts", "import { allocate } from './api';\n\nexport function pick(qty: number) {\n  const n = allocate(qty);\n  return n;\n}\n");
    origin.write("src/Bin.vue", "<template>\n  <div>{{ label }}</div>\n</template>\n<script setup lang=\"ts\">\nconst label = 'bin';\n</script>\n");
    origin.write("views/bin.html.twig", "{% set label = 'x' %}\n<p>{{ label|upper }}</p>\n");
    origin.commit("feature");
    origin.publish_pr(1, "feature");

    let data = tempfile::tempdir().unwrap();
    let service = PrService::new(
        GitHubClient::new("http://127.0.0.1:9", "t").unwrap(),
        RepoStore::new(data.path().join("repos"), None),
        Cache::open(&data.path().join("cache.sqlite")).unwrap(),
        Highlighter::new(),
    );
    let snapshot = service.fetch(Stack { prs: vec![pull_request(1, "main", "feature", &origin.url())], focus: 0 }).unwrap();
    (origin, service, snapshot, data)
}

fn spots(hits: &[wispy_core::symbols::Hit]) -> Vec<(String, u32)> {
    hits.iter().map(|h| (h.path.clone(), h.line)).collect()
}

#[test]
fn finds_definitions_and_usages_split_by_changed_lines() {
    let (_origin, service, snapshot, _data) = setup();
    let view = service.range(&snapshot, 0, 0).unwrap();
    let index = service.symbol_index(&snapshot, &view).unwrap();

    let allocate = index.usages("allocate");
    assert_eq!(spots(&allocate.definitions), vec![("src/Allocator.php".into(), 4)]);
    assert_eq!(spots(&allocate.changed), vec![("src/Picker.ts".into(), 4)], "the call on the changed line");
    assert_eq!(spots(&allocate.unchanged), vec![("src/Picker.ts".into(), 1)], "the import; Unrelated.php isn't in the diff");
    assert_eq!(allocate.changed[0].col, 12);
    assert_eq!(allocate.changed[0].text, "const n = allocate(qty);");

    // PHP variables are found by name, with or without the $.
    let qty = index.usages("$qty");
    assert_eq!(qty.name, "qty");
    assert!(qty.changed.iter().any(|h| h.path == "src/Allocator.php" && h.line == 6));

    let label = index.usages("label");
    let definitions = spots(&label.definitions);
    assert!(definitions.contains(&("src/Bin.vue".into(), 5)), "Vue script: {definitions:?}");
    assert!(definitions.contains(&("views/bin.html.twig".into(), 1)), "Twig set: {definitions:?}");
    let uses: Vec<(String, u32)> = spots(&label.changed);
    assert!(uses.contains(&("src/Bin.vue".into(), 2)), "template interpolation: {uses:?}");
    assert!(uses.contains(&("views/bin.html.twig".into(), 2)));

    assert!(index.usages("nothing_like_this").definitions.is_empty());
}

#[test]
fn greps_the_whole_repo_after_downloading_its_blobs_in_one_batch() {
    let (_origin, service, snapshot, data) = setup();
    let git = wispy_core::git::Git::new(data.path().join("repos/acme/shop.git"), None);
    let missing = |git: &wispy_core::git::Git| {
        git.run_string(&["rev-list", "--objects", "--missing=print", "--no-walk", &snapshot.heads[0]]).unwrap().lines().filter(|l| l.starts_with('?')).count()
    };
    assert!(missing(&git) > 0, "blobless before the first search");

    let mut batches = Vec::new();
    let summary = service.grep(&snapshot, 0, "allocate", true, |hits| {
        batches.push(hits);
        true
    }).unwrap();
    assert_eq!(missing(&git), 0);
    let mut hits: Vec<(String, u32)> = batches.concat().into_iter().map(|h| (h.path, h.line)).collect();
    hits.sort();
    assert_eq!(summary.total, hits.len());
    assert_eq!(summary.unsearched, 0);
    assert_eq!(
        hits,
        vec![
            ("src/Allocator.php".into(), 4),
            ("src/Picker.ts".into(), 1),
            ("src/Picker.ts".into(), 4),
            ("src/Unrelated.php".into(), 2),
        ]
    );
    assert_eq!(batches[0].len(), 1, "the first hit is sent on its own, immediately");

    // Whole-word matching, and stopping early.
    let mut partial = 0;
    service.grep(&snapshot, 0, "alloc", true, |h| { partial += h.len(); true }).unwrap();
    assert_eq!(partial, 0);
    let mut seen = 0;
    service.grep(&snapshot, 0, "allocate", true, |h| { seen += h.len(); false }).unwrap();
    assert_eq!(seen, 1);
}

#[test]
fn greps_the_downloaded_files_when_offline_and_counts_the_rest() {
    let (origin, service, snapshot, _data) = setup();
    // Viewing the range downloads the changed files; Unrelated.php stays blobless.
    service.range(&snapshot, 0, 0).unwrap();
    let gone = origin.path().with_extension("offline");
    std::fs::rename(origin.path(), &gone).unwrap();

    let mut hits = Vec::new();
    let summary = service.grep(&snapshot, 0, "allocate", true, |h| { hits.extend(h); true }).unwrap();
    std::fs::rename(&gone, origin.path()).unwrap();

    let mut spots: Vec<(String, u32)> = hits.into_iter().map(|h| (h.path, h.line)).collect();
    spots.sort();
    assert_eq!(spots, vec![("src/Allocator.php".into(), 4), ("src/Picker.ts".into(), 1), ("src/Picker.ts".into(), 4)]);
    assert_eq!(summary.total, 3);
    assert_eq!(summary.unsearched, 1, "Unrelated.php was never downloaded");
}

#[test]
fn reads_any_file_of_the_head_and_locates_head_lines_in_a_view() {
    let (_origin, service, snapshot, _data) = setup();
    let lines = service.read_file(&snapshot, 0, "src/Unrelated.php").unwrap();
    assert_eq!(lines.len(), 2);
    let second: String = lines[1].iter().map(|s| s.1.as_str()).collect();
    assert_eq!(second, "// mentions allocate only here");

    let view = service.range(&snapshot, 0, 0).unwrap();
    let file = view.summary.files.iter().position(|f| f.path == "src/Allocator.php").unwrap();
    let changed = locate_new_line(&view, file, 6).unwrap();
    assert!(changed.unified.is_some() && changed.split.is_some());
    let far = locate_new_line(&view, file, 1).unwrap();
    assert_eq!(far.unified, None, "line 1 is outside the hunk's context");
    assert_eq!(far.split, Some(1));
}
