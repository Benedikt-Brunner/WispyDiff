mod support;

use support::OriginRepo;
use wispy_core::highlight::Highlighter;
use wispy_core::highlight_cache::HighlightCache;
use wispy_core::noise::path_noise;
use wispy_core::range::{compute_range, RangeSpec};
use wispy_core::repo_store::RepoStore;

#[test]
fn recognizes_lockfiles_minified_files_and_snapshots_by_path() {
    assert_eq!(path_noise("composer.lock"), Some("lockfile"));
    assert_eq!(path_noise("src/Resources/app/administration/package-lock.json"), Some("lockfile"));
    assert_eq!(path_noise("pnpm-lock.yaml"), Some("lockfile"));
    assert_eq!(path_noise("public/app.min.js"), Some("minified"));
    assert_eq!(path_noise("public/app.js.map"), Some("minified"));
    assert_eq!(path_noise("tests/__snapshots__/Stock.test.ts.snap"), Some("snapshot"));
    assert_eq!(path_noise("src/Lock.php"), None);
    assert_eq!(path_noise("composer.json"), None);
}

#[test]
fn marks_files_generated_by_the_head_gitattributes() {
    let origin = OriginRepo::init();
    origin.write("src/Api.php", "<?php\n");
    let base = origin.commit("base");
    origin.write(".gitattributes", "src/Generated/** linguist-generated\n*.pb.php linguist-generated=true\n");
    origin.write("src/Generated/Client.php", "<?php\n// generated\n");
    origin.write("src/Proto/Order.pb.php", "<?php\n");
    origin.write("src/Api.php", "<?php\n// real change\n");
    origin.write("composer.lock", "{}\n");
    let head = origin.commit("regenerate");
    origin.publish_pr(1, "main");

    let root = tempfile::tempdir().unwrap();
    let store = RepoStore::new(root.path(), None);
    let git = store.ensure_repo("acme", "shop", &origin.url()).unwrap();
    store.fetch_pr(&git, 1, "main").unwrap();
    let spec = RangeSpec { from: base, heads: vec![head], prs: vec![0], ignore_whitespace: false };
    let view = compute_range(&git, &spec, &Highlighter::new(), &HighlightCache::default()).unwrap();

    let noise: Vec<(&str, Option<&str>)> =
        view.summary.files.iter().map(|f| (f.path.as_str(), f.noise.as_deref())).collect();
    assert_eq!(
        noise,
        vec![
            (".gitattributes", None),
            ("composer.lock", Some("lockfile")),
            ("src/Api.php", None),
            ("src/Generated/Client.php", Some("generated")),
            ("src/Proto/Order.pb.php", Some("generated")),
        ]
    );
}
