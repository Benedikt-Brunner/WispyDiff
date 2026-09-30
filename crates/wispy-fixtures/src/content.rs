//! Deterministic, plausible-looking source files in the languages WispyDiff targets.

use std::path::Path;

const WORDS: &[&str] = &[
    "stock", "bin", "order", "picking", "warehouse", "shipment", "batch", "location", "supplier", "invoice",
    "return", "capacity", "lot", "barcode", "carrier", "delivery", "product", "quantity", "reservation", "transfer",
];

/// xorshift64*: tiny, fast, and stable across platforms and Rust versions.
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Self {
        Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1)
    }

    pub fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    pub fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }

    fn word(&mut self) -> &'static str {
        WORDS[self.below(WORDS.len() as u64) as usize]
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Lang {
    Php,
    Ts,
    Vue,
    Twig,
}

struct Line {
    text: String,
    /// Statement lines can be re-rendered by [`SourceFile::mutate`] without breaking syntax.
    statement: Option<(Lang, u32)>,
}

pub struct SourceFile {
    path: String,
    lines: Vec<Line>,
}

impl SourceFile {
    pub fn generate(lang: Lang, index: u32, target_lines: usize, rng: &mut Rng) -> Self {
        let path = match (lang, index) {
            (Lang::Php, 10_000) => "src/Allocation/PickListAllocator.php".to_string(),
            (Lang::Php, i) => format!("src/Module{}/Service{i}.php", i % 12),
            (Lang::Ts, i) => format!("src/Resources/app/administration/src/module/m{}/helper{i}.ts", i % 10),
            (Lang::Vue, i) => format!("src/Resources/app/storefront/src/component/c{}/Component{i}.vue", i % 10),
            (Lang::Twig, i) => format!("src/Resources/views/storefront/page/p{}/template{i}.html.twig", i % 8),
        };
        let mut lines: Vec<Line> = header(lang, index).into_iter().map(fixed).collect();
        let footer: Vec<Line> = footer(lang).into_iter().map(fixed).collect();
        let mut block = 0u32;
        while lines.len() + footer.len() < target_lines {
            lines.extend(block_lines(lang, block, rng));
            block += 1;
        }
        lines.extend(footer);
        SourceFile { path, lines }
    }

    pub fn mutate(&mut self, count: usize, rng: &mut Rng) {
        let statements: Vec<usize> = (0..self.lines.len()).filter(|&i| self.lines[i].statement.is_some()).collect();
        for _ in 0..count.min(statements.len()) {
            let index = statements[rng.below(statements.len() as u64) as usize];
            let (lang, block) = self.lines[index].statement.expect("statement line");
            self.lines[index].text = statement(lang, block, rng);
        }
    }

    pub fn write(&self, root: &Path) {
        let path = root.join(&self.path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let mut text = self.lines.iter().map(|l| l.text.as_str()).collect::<Vec<_>>().join("\n");
        text.push('\n');
        std::fs::write(path, text).unwrap();
    }
}

fn fixed(text: String) -> Line {
    Line { text, statement: None }
}

fn header(lang: Lang, index: u32) -> Vec<String> {
    match lang {
        Lang::Php => vec![
            "<?php declare(strict_types=1);".into(),
            String::new(),
            format!("namespace Wispy\\Fixture\\Module{};", index % 12),
            String::new(),
            "use Wispy\\Fixture\\Support\\Helper;".into(),
            String::new(),
            format!("final class Service{index}"),
            "{".into(),
            "    public function __construct(private readonly Helper $helper) {}".into(),
        ],
        Lang::Ts => vec![format!("import {{ helper }} from '../../support/helper';"), String::new()],
        Lang::Vue => vec!["<script setup lang=\"ts\">".into(), "import { computed, ref } from 'vue';".into(), String::new()],
        Lang::Twig => vec![format!("{{% extends '@Storefront/storefront/base.html.twig' %}}"), String::new()],
    }
}

fn footer(lang: Lang) -> Vec<String> {
    match lang {
        Lang::Php => vec!["}".into()],
        Lang::Ts | Lang::Twig => vec![],
        Lang::Vue => vec!["</script>".into()],
    }
}

fn block_lines(lang: Lang, block: u32, rng: &mut Rng) -> Vec<Line> {
    let stmt = |rng: &mut Rng| Line { text: statement(lang, block, rng), statement: Some((lang, block)) };
    let word = rng.word();
    let mut lines = match lang {
        Lang::Php => vec![
            fixed(String::new()),
            fixed(format!("    /** Computes the {word} allocation for block {block}. */")),
            fixed(format!("    public function {word}{block}(int $value): int")),
            fixed("    {".into()),
            fixed("        $result = $value;".into()),
        ],
        Lang::Ts => vec![fixed(format!("export function {word}{block}(value: number): number {{")), fixed("  let result = value;".into())],
        Lang::Vue => vec![fixed(format!("const {word}{block} = computed(() => {{")), fixed("  let result = 0;".into())],
        Lang::Twig => vec![fixed(format!("{{% block {word}_{block} %}}"))],
    };
    for _ in 0..4 {
        lines.push(stmt(rng));
    }
    lines.extend(match lang {
        Lang::Php => vec![fixed("        return $result;".into()), fixed("    }".into())],
        Lang::Ts | Lang::Vue => vec![fixed("  return result;".into()), fixed(if lang == Lang::Vue { "});" } else { "}" }.into()), fixed(String::new())],
        Lang::Twig => vec![fixed("{% endblock %}".into()), fixed(String::new())],
    });
    lines
}

fn statement(lang: Lang, block: u32, rng: &mut Rng) -> String {
    let (word, other, n, m) = (rng.word(), rng.word(), rng.below(1000), rng.below(97) + 1);
    match (lang, rng.below(4)) {
        (Lang::Php, 0) => format!("        $result += $this->helper->compute('{word}', {n});"),
        (Lang::Php, 1) => format!("        $result = $result > {n} ? $result - {m} : $result * {m}; // {word} threshold"),
        (Lang::Php, 2) => format!("        $result ^= \\strlen('{word}_{other}') + {n};"),
        (Lang::Php, _) => format!("        $result = max($result, {n}); # clamp {word}{block}"),
        (Lang::Ts | Lang::Vue, 0) => format!("  result += helper('{word}', {n});"),
        (Lang::Ts | Lang::Vue, 1) => format!("  result = result > {n} ? result - {m} : result * {m}; // {word} threshold"),
        (Lang::Ts | Lang::Vue, 2) => format!("  result ^= '{word}_{other}'.length + {n};"),
        (Lang::Ts | Lang::Vue, _) => format!("  result = Math.max(result, {n}); /* clamp {word}{block} */"),
        (Lang::Twig, 0) => format!("    <div class=\"{word}-{other}\">{{{{ {word}.{other}|default({n}) }}}}</div>"),
        (Lang::Twig, 1) => format!("    {{% if {word}.quantity > {n} %}}<span>{{{{ '{other}'|trans }}}}</span>{{% endif %}}"),
        (Lang::Twig, 2) => format!("    {{# {word} {other} {n} #}}"),
        (Lang::Twig, _) => format!("    <p data-{word}=\"{n}\">{{{{ {other}|length * {m} }}}}</p>"),
    }
}
