# Intra-line (word-level) diff highlighting: how other viewers do it

Research note, 2026-10-09. Question: how do other diff viewers decide which parts of a changed line to
emphasise, and is there a better approach than the one in `crates/wispy-core/src/words.rs`?

Source permalinks are pinned to the commits that were current when this was written:
delta `3c2269c`, git `6de20f6`, diff-match-patch `62f2e68`, diff-so-fancy `7d31167`, GitLab `7a83a0e`
(GitHub mirror `gitlabhq/gitlabhq`), difftastic `8a26f10`, VS Code `5042306`, IntelliJ `1ca4148`,
Gerrit `1820128`, Phabricator `5720a38` / Arcanist `e50d1bc`, Meld `8317836`. Claims are cited to
source lines. **Unverified** marks anything I could not confirm from a primary source.

## Summary

- There are two families.
  - **Line-pair tools** compare one deleted line with one added line: diff-highlight, diff-so-fancy,
    GitLab, Phabricator, delta, and WispyDiff today.
  - **Whole-block tools** diff the entire run of deleted text against the entire run of added text as
    one sequence with newlines in it, then cut the result back into per-line ranges: VS Code, JetBrains,
    Gerrit, Meld, `git diff --word-diff`, and difftastic's text fallback. Editors and Gerrit use whole
    blocks. Pager tools mostly pair lines, because they stream output.
- **Pairing of line-pair tools.** Most pair positionally, and only when the deleted and added counts are
  equal: diff-highlight, diff-so-fancy, GitLab. Phabricator pairs positionally always. Only delta pairs
  by similarity: a greedy look-ahead that accepts the first added line within a distance threshold
  (`--max-line-distance`, default 0.6).
- **Our suppression ratio is delta's distance.** WispyDiff's suppression ratio, changed / (changed +
  2·unchanged) with whitespace ignored, is the same metric as delta's line distance, and our 60% cutoff
  equals delta's default. The difference: delta uses it to choose *which* lines to pair, while we only
  use it to switch highlighting off.
- **Granularity.** It ranges from characters (diff-highlight, GitLab, Gerrit, Phabricator, Meld, VS Code)
  to words plus single punctuation characters (delta, difftastic, WispyDiff) to "words only, then
  punctuation in a second pass" (JetBrains). Only difftastic is syntax-aware, through a tree-sitter tree
  diff with Dijkstra.
- **Noise suppression comes in four flavours.**
  1. *Merge across small equal gaps*: Gerrit ≤5 chars, Meld <3, VS Code ≤2, Phabricator ≤3 (as
     smoothing), VS Code legacy <3.
  2. *Semantic cleanup*: diff-match-patch drops equalities no longer than the edits on both sides, which
     GitLab uses. VS Code has a more elaborate version with boundary scores and word snapping.
  3. *Whole-pair or whole-block gates*: delta's distance, difftastic's "more than 2 shared words and at
     least half shared", diff-highlight's "don't highlight if the whole line would be".
  4. *Caps that give up*: Phabricator 100 glyphs, Meld 20,000 chars, difftastic 1,000 words per block,
     VS Code/Gerrit 5 s timeouts, JetBrains Myers threshold then a patience fallback.
- **Our weak case is structural.** The weak case (`sendMail: false,` opposite `return { orderId,
  warehouseId, sendMail: false };`) can't be fixed by any line-pair method: the new line draws its tokens
  from several deleted lines. A scratch simulation (below) shows two things. A whole-block token diff
  anchored on rare tokens (patience/histogram style) leaves that `return` line with **no** highlights.
  The two genuinely new `const` lines come out fully changed. A plain-LCS block diff only partly fixes it.

**Top recommendations**, detailed at the end:

1. Diff each change block as one token stream, using a histogram/patience-style algorithm such as
   `imara-diff`, and map the result back to per-line UTF-16 spans.
2. Add semantic cleanup on the token script: absorb chaff equalities and short punctuation gaps.
3. Replace the per-pair 60% gate with a per-block similarity gate plus "a fully changed line gets no
   emphasis".

Keep the current pairwise code as the fallback for blocks over the token cap. Positional side-by-side
placement can stay as it is, the way Gerrit does it.

---

## Per-tool findings

### git `contrib/diff-highlight` (and diff-so-fancy, which embeds it)

1. **Pairing.** Positional and one-to-one, only within a hunk whose removed and added counts are equal.
   Otherwise both sides print unhighlighted. The code comment admits it: "we could try to be clever and
   match up similar lines. But for now we are simple and stupid"
   ([DiffHighlight.pm L151-L176](https://github.com/git/git/blob/6de20f6092dcf9bdb1c8efe03db4b70c82b423dd/contrib/diff-highlight/DiffHighlight.pm#L151-L176)).
   The README calls positional pairing "simple and tends to work well in practice". It documents the
   pathological "line removed at the top, added at the bottom" case and suggests "pre-matching the lines
   into pairs according to some heuristic"
   ([README L13-L25, L205-L248](https://github.com/git/git/blob/6de20f6092dcf9bdb1c8efe03db4b70c82b423dd/contrib/diff-highlight/README#L13-L25)).
2. **Granularity.** Characters. Each line is split into single characters, with ANSI colour sequences
   kept as tokens and UTF-8 decoded when valid
   ([`split_line` L262-L270](https://github.com/git/git/blob/6de20f6092dcf9bdb1c8efe03db4b70c82b423dd/contrib/diff-highlight/DiffHighlight.pm#L262-L270)).
3. **Inner algorithm.** Not a diff at all. It takes the common prefix and the common suffix, and
   highlights the single middle span
   ([`highlight_pair` L201-L250](https://github.com/git/git/blob/6de20f6092dcf9bdb1c8efe03db4b70c82b423dd/contrib/diff-highlight/DiffHighlight.pm#L201-L250)).
   The README's rationale: "the point of the highlight is to call attention to a certain area … it ends
   up being more readable to just have a single blob". It also shows why a per-character LCS without
   boundaries produces junk (`-{i}nt-{ere}sti-{ng}`), and says doing it right "would probably involve a
   set of content-specific boundary patterns, similar to word-diff"
   ([README L27-L34, L168-L200](https://github.com/git/git/blob/6de20f6092dcf9bdb1c8efe03db4b70c82b423dd/contrib/diff-highlight/README#L168-L200)).
4. **Noise heuristics.** `is_pair_interesting` skips a pair when the prefix or suffix is only the `+`/`-`
   marker plus whitespace. In effect, "don't highlight if the whole line would be highlighted"
   ([L303-L325](https://github.com/git/git/blob/6de20f6092dcf9bdb1c8efe03db4b70c82b423dd/contrib/diff-highlight/DiffHighlight.pm#L303-L325)).
5. **Caps.** None explicit. It runs in O(line length).

**diff-so-fancy** loads `DiffHighlight.pm` and runs every line through it first
([diff-so-fancy L14-L16, L119-L121](https://github.com/so-fancy/diff-so-fancy/blob/7d311672c2f4f82020e26cd3de5c3a765e57762b/diff-so-fancy#L119-L121)).
Its vendored `lib/DiffHighlight.pm` is byte-identical to git's at the pinned commits, compared locally
with `diff`. So everything above applies unchanged.

### `git diff --word-diff` / `--color-words`

1. **Pairing.** None. It is a **whole-block** approach. All `-` lines of a change run are appended into
   one buffer and all `+` lines into another. The run is flushed when a context line or hunk header
   arrives
   ([diff.c `diff_words_append` L1969-L1979, flush at L2463-L2466 and L2494](https://github.com/git/git/blob/6de20f6092dcf9bdb1c8efe03db4b70c82b423dd/diff.c#L2463-L2496)).
   The docs say it "operat[es] by taking the same line-by-line diff that is produced without the option
   and computing word-by-word changes within each hunk"
   ([diff-options.adoc L461-L467](https://github.com/git/git/blob/6de20f6092dcf9bdb1c8efe03db4b70c82b423dd/Documentation/diff-options.adoc#L461-L467)).
2. **Granularity.** Words. By default a word is a run of non-whitespace. `--word-diff-regex` changes
   that, and anything between matches "is considered whitespace and ignored(!)"
   ([diff-options.adoc L435-L492](https://github.com/git/git/blob/6de20f6092dcf9bdb1c8efe03db4b70c82b423dd/Documentation/diff-options.adoc#L469-L488)).
   Diff drivers ship per-language word regexes, and git appends `|[^[:space:]]` so every non-space
   character is at least a token
   ([userdiff.c L15-L24](https://github.com/git/git/blob/6de20f6092dcf9bdb1c8efe03db4b70c82b423dd/userdiff.c#L15-L24);
   Rust example
   [L340-L345](https://github.com/git/git/blob/6de20f6092dcf9bdb1c8efe03db4b70c82b423dd/userdiff.c#L340-L345)).
3. **Inner algorithm.** Each buffer is rewritten as one word per line (`diff_words_fill`). Word offsets
   are kept in `orig[]`, and the two word-lists are run through xdiff with `xpp.flags = 0`, which is
   git's default Myers, and zero context
   ([diff.c L2191-L2225, L2227-L2279](https://github.com/git/git/blob/6de20f6092dcf9bdb1c8efe03db4b70c82b423dd/diff.c#L2227-L2279)).
   **Mapping back:** the hunk callback (`fn_out_diff_words_aux`, L2098) translates word indices to byte
   ranges in the original text through `orig[]`. Newlines are part of that text, so the output stream
   keeps its line structure. There is no side-by-side mapping, because output is one merged stream.
4. **Noise heuristics.** None beyond tokenization. Ignoring inter-word whitespace is a design choice.
5. **Caps.** None specific to word diff.

### delta (dandavison/delta)

1. **Pairing.** **Similarity-based, greedy, with unbounded look-ahead.** For each minus line in order,
   delta scans the remaining plus lines. It accepts the first one whose distance is `<=
   max_line_distance`. Plus lines it skipped are emitted unpaired, and a minus line with no match is
   unpaired
   ([edits.rs L46-L103](https://github.com/dandavison/delta/blob/3c2269c6b845913965575f11b0196d70ef5352f3/src/edits.rs#L46-L103)).
   A separate threshold, `max_line_distance_for_naively_paired_lines`, applies only when the counts are
   equal ([L64-L66](https://github.com/dandavison/delta/blob/3c2269c6b845913965575f11b0196d70ef5352f3/src/edits.rs#L64-L66)).
   It comes from an experimental env setting and defaults to 0.0
   ([config.rs L194-L199](https://github.com/dandavison/delta/blob/3c2269c6b845913965575f11b0196d70ef5352f3/src/config.rs#L194-L199)).
   `--max-line-distance` defaults to **0.6**
   ([cli.rs L594-L600](https://github.com/dandavison/delta/blob/3c2269c6b845913965575f11b0196d70ef5352f3/src/cli.rs#L594-L600)).
2. **Granularity.** Matches of `--word-diff-regex`, default `\w+`, are single tokens. All text between
   matches is split into individual grapheme clusters
   ([edits.rs `tokenize` L140-L165](https://github.com/dandavison/delta/blob/3c2269c6b845913965575f11b0196d70ef5352f3/src/edits.rs#L140-L165);
   [cli.rs L915-L921](https://github.com/dandavison/delta/blob/3c2269c6b845913965575f11b0196d70ef5352f3/src/cli.rs#L915-L921)).
   That is the same as our tokenizer, except that we keep whitespace runs as one token.
3. **Inner algorithm.** A full O(n·m) Wagner–Fischer edit-distance table over tokens. Deletion and
   insertion cost 2, there is no substitution, and starting a new run of changes costs an extra 1
   (`INITIAL_MISMATCH_PENALTY`). Ties prefer insertion, then deletion, then match, "in order to group
   changes together"
   ([align.rs L4-L7, L55-L125](https://github.com/dandavison/delta/blob/3c2269c6b845913965575f11b0196d70ef5352f3/src/align.rs#L55-L125)).
   **Distance** is computed while annotating. Numerator: the trimmed display width of deleted plus
   inserted sections. Denominator: that same width plus **2×** the unchanged width
   ([edits.rs L219-L289](https://github.com/dandavison/delta/blob/3c2269c6b845913965575f11b0196d70ef5352f3/src/edits.rs#L219-L289)).
   That is changed / (changed + 2·unchanged), which is our `MAX_CHANGED` ratio.
4. **Noise heuristics.**
   - The run-opening penalty favours fewer, longer runs.
   - A whitespace-only unchanged section between a deletion and an insertion takes the edit's style. This
     is the same as our whitespace bridging
     ([edits.rs L236-L254](https://github.com/dandavison/delta/blob/3c2269c6b845913965575f11b0196d70ef5352f3/src/edits.rs#L236-L254)).
   - Trailing whitespace is split off.
   - Unpaired lines get no emphasis at all, so the distance threshold works as the "too different"
     gate.
5. **Caps.**
   - `--line-buffer-size` (default 32) bounds how many nearby lines are buffered for comparison. The help
     text warns that raising it hurts performance on whole-file additions
     ([cli.rs L482-L490](https://github.com/dandavison/delta/blob/3c2269c6b845913965575f11b0196d70ef5352f3/src/cli.rs#L482-L490)).
   - `--max-line-length` (default 3000) truncates lines
     ([L613-L618](https://github.com/dandavison/delta/blob/3c2269c6b845913965575f11b0196d70ef5352f3/src/cli.rs#L613-L618)).
   - Look-ahead is O(minus × plus) alignments, each O(n·m).
   - **Unverified:** exactly how `line_buffer_size` interacts with the minus/plus buffers passed to
     `infer_edits`. I didn't trace it beyond the help text.

### Google diff-match-patch (and GitLab, which uses it)

1. **Pairing.** Not applicable. It is a string-to-string library.
2. **Granularity.** Characters by default. The wiki shows a "line mode" and a "word mode": map each line
   or word to one Unicode character with `diff_linesToChars_`, diff, then map back
   ([wiki: Line or Word Diffs](https://github.com/google/diff-match-patch/wiki/Line-or-Word-Diffs);
   [`diff_linesToChars_` L466](https://github.com/google/diff-match-patch/blob/62f2e689f498f9c92dbc588c58750addec9b1654/javascript/diff_match_patch_uncompressed.js#L466)).
3. **Inner algorithm.** Pre-passes: equality check, common prefix/suffix, single-edit cases, and a
   "half match". Half match is a shared substring at least half the longer text, and is skipped when
   there is no timeout because "this speedup can produce non-minimal diffs"
   ([L655-L680](https://github.com/google/diff-match-patch/blob/62f2e689f498f9c92dbc588c58750addec9b1654/javascript/diff_match_patch_uncompressed.js#L655-L680)).
   Line mode runs automatically for texts over 100 chars
   ([L228](https://github.com/google/diff-match-patch/blob/62f2e689f498f9c92dbc588c58750addec9b1654/javascript/diff_match_patch_uncompressed.js#L228)).
   The core is Myers' bisect (`diff_bisect_`, L316). The author's paper describes all of these
   ([Neil Fraser, *Diff Strategies*, §1, §2.3](https://neil.fraser.name/writing/diff/)).
4. **Noise heuristics.** This is the canonical reference for cleanup.
   - **`diff_cleanupSemantic`** removes "semantic chaff". An equality whose length is ≤ the larger of
     the insertions/deletions on *both* sides becomes a delete plus an insert. It then merges, and
     re-checks the previous equality
     ([L760-L870, rule at L788-L792](https://github.com/google/diff-match-patch/blob/62f2e689f498f9c92dbc588c58750addec9b1654/javascript/diff_match_patch_uncompressed.js#L788-L812)).
     It then pulls out overlaps between adjacent delete/insert pairs when an overlap is ≥ half of either
     edit ([L821-L866](https://github.com/google/diff-match-patch/blob/62f2e689f498f9c92dbc588c58750addec9b1654/javascript/diff_match_patch_uncompressed.js#L821-L866)).
   - **`diff_cleanupSemanticLossless`** slides each single edit sideways to the position with the best
     boundary score. Scores: 6 at an edge, 5 for a blank line, 4 for a line break, 3 for end of
     sentence, 2 for whitespace, 1 for non-alphanumeric
     ([L875-L998](https://github.com/google/diff-match-patch/blob/62f2e689f498f9c92dbc588c58750addec9b1654/javascript/diff_match_patch_uncompressed.js#L875-L940)).
   - **`diff_cleanupEfficiency`** is for machine use. It splits equalities shorter than `Diff_EditCost`
     (4) when surrounded by edits
     ([L1005-L1060](https://github.com/google/diff-match-patch/blob/62f2e689f498f9c92dbc588c58750addec9b1654/javascript/diff_match_patch_uncompressed.js#L1005-L1060)).
5. **Caps.** `Diff_Timeout = 1.0` s, after which it returns a valid but non-minimal diff
   ([L33-L37](https://github.com/google/diff-match-patch/blob/62f2e689f498f9c92dbc588c58750addec9b1654/javascript/diff_match_patch_uncompressed.js#L33-L37)).

### GitLab

1. **Pairing.** Positional, and only within **runs of exactly N deleted followed by N added lines**.
   `PairSelector` builds a string of line prefixes (`" - +  -+  ---+++"`). A recursive regex matches
   balanced `-…+` runs bounded by context, and pairs line *i* with line *i+N*
   ([pair_selector.rb L8-L46](https://github.com/gitlabhq/gitlabhq/blob/7a83a0eef0b168e3b72b3de22f50b0c055c8faff/lib/gitlab/diff/pair_selector.rb#L8-L46)).
   Unequal runs get no inline diff.
2. **Granularity.** Characters, after the leading `+`/`-` is skipped with `offset: 1`
   ([highlight.rb L54-L66](https://github.com/gitlabhq/gitlabhq/blob/7a83a0eef0b168e3b72b3de22f50b0c055c8faff/lib/gitlab/diff/highlight.rb#L54-L66)).
3. **Inner algorithm.** `DiffMatchPatch#diff_main`, then `diff_cleanupSemantic`. Delete and insert ops
   become marker ranges
   ([char_diff.rb L14-L46](https://github.com/gitlabhq/gitlabhq/blob/7a83a0eef0b168e3b72b3de22f50b0c055c8faff/lib/gitlab/diff/char_diff.rb#L14-L46)).
   The library is a vendored `diff_match_patch` gem with a 1 s default timeout and edit cost 4
   ([vendor/gems/diff_match_patch/lib/diff_match_patch.rb L14-L21](https://github.com/gitlabhq/gitlabhq/blob/7a83a0eef0b168e3b72b3de22f50b0c055c8faff/vendor/gems/diff_match_patch/lib/diff_match_patch.rb#L14-L21);
   [Gemfile L291](https://github.com/gitlabhq/gitlabhq/blob/7a83a0eef0b168e3b72b3de22f50b0c055c8faff/Gemfile#L291)).
4. **Noise heuristics.** dmp semantic cleanup. An inline diff is skipped when an empty line was replaced
   with content
   ([inline_diff.rb L14-L19](https://github.com/gitlabhq/gitlabhq/blob/7a83a0eef0b168e3b72b3de22f50b0c055c8faff/lib/gitlab/diff/inline_diff.rb#L14-L19)).
   I found no "too different" gate in these files.
5. **Caps.** The dmp timeout only, in the code read.
   - **Unverified:** whether the newer "Rapid Diffs" frontend changes any of this. I only read the Ruby
     backend.
   - The open issue [gitlab#285464](https://gitlab.com/gitlab-org/gitlab/-/issues/285464) complains that
     intraline highlighting is weak for prose and cites Gerrit as better.

### GitHub (closed source)

- **Documented.** A 2014 blog post by Adam Roben: "Commits, compare views, and pull requests now
  highlight individual changed words instead of the entire changed section", and it works in split view
  ([github.blog, 2014-09-04](https://github.blog/news-insights/better-word-highlighting-in-diffs)).
  The post gives no algorithm, pairing rule or threshold.
- **Documented limits.** These apply to the diff as a whole, not to word highlighting. No total PR diff
  may exceed 20,000 loadable lines or 1 MB of raw diff. No single file may exceed 20,000 lines or
  500 KB. 400 lines / 20 KB load automatically, and at most 300 files are shown
  ([docs: Repository limits](https://docs.github.com/en/repositories/creating-and-managing-repositories/repository-limits)).
- **Unverified (everything else).** I found no primary source for how GitHub pairs lines, its
  tokenization, its inner algorithm, or when it suppresses emphasis. Any statement about GitHub's
  behaviour beyond the two items above would be inference from observing the UI. I have not recorded
  any.

### difftastic

1. **Pairing.** No line pairing in syntax mode. It is a structural diff over the whole file.
   - It models the diff as a shortest path through a graph whose vertices are positions in the two
     syntax trees, and solves it with Dijkstra
     ([manual: Diffing](https://github.com/Wilfred/difftastic/blob/8a26f10e77f2a1d1a56b35bbc29c60f4c2ab1ea2/manual/src/diffing.md)).
   - Before that, obviously-unchanged nodes are peeled off with a linear diff
     ([unchanged.rs L1-L2](https://github.com/Wilfred/difftastic/blob/8a26f10e77f2a1d1a56b35bbc29c60f4c2ab1ea2/src/diff/unchanged.rs#L1-L2)).
   - **Display alignment** is derived from matches. Each unchanged token knows its opposite position. The
     side-by-side places a line opposite the next monotonic line that holds its matched tokens
     ([display/hunks.rs `next_opposite` L537, `matched_novel_lines` L556](https://github.com/Wilfred/difftastic/blob/8a26f10e77f2a1d1a56b35bbc29c60f4c2ab1ea2/src/display/hunks.rs#L537-L603)).
2. **Granularity.** Syntax nodes from tree-sitter. Inside comments and strings it falls back to words:
   alphanumeric/`_` runs and single other characters
   ([syntax.rs `split_atom_words` L736](https://github.com/Wilfred/difftastic/blob/8a26f10e77f2a1d1a56b35bbc29c60f4c2ab1ea2/src/parse/syntax.rs#L736);
   [words.rs](https://github.com/Wilfred/difftastic/blob/8a26f10e77f2a1d1a56b35bbc29c60f4c2ab1ea2/src/words.rs#L1-L46)).
   The **text fallback** (unsupported language or over the limits) is line-diff first. Each run of
   novel lines is joined into one string and word-diffed **as a whole block, across lines**
   ([line_parser.rs L74-L98, L165-L269](https://github.com/Wilfred/difftastic/blob/8a26f10e77f2a1d1a56b35bbc29c60f4c2ab1ea2/src/line_parser.rs#L165-L269)).
   Positions map back to (line, column) spans with `LinePositions::from_region`.
3. **Inner algorithm.**
   - Dijkstra with edge costs. An unchanged node costs 1 plus a depth term, plus 200 if it is
     punctuation, "better to have unchanged variable names and novel punctuation than the reverse". A
     novel atom or delimiter costs 300. A replaced string or comment costs 500 + (100 − Levenshtein %)
     ([graph.rs L323-L385](https://github.com/Wilfred/difftastic/blob/8a26f10e77f2a1d1a56b35bbc29c60f4c2ab1ea2/src/diff/graph.rs#L323-L385)).
   - Linear diffs (lines and words) use **Histogram** via `imara-diff`
     ([lcs_diff.rs L1-L8, L34](https://github.com/Wilfred/difftastic/blob/8a26f10e77f2a1d1a56b35bbc29c60f4c2ab1ea2/src/diff/lcs_diff.rs#L1-L34)).
4. **Noise heuristics.** For strings and comments, `has_common_words` requires more than 2 unchanged
   words and `unchanged*2 >= novel`. Otherwise the whole atom is shown as novel
   ([syntax.rs L825-L850](https://github.com/Wilfred/difftastic/blob/8a26f10e77f2a1d1a56b35bbc29c60f4c2ab1ea2/src/parse/syntax.rs#L825-L850)).
   The manual catalogues the hard cases: punctuation atoms, "replacements with minor similarities", and
   small changes to large strings
   ([tricky_cases.md](https://github.com/Wilfred/difftastic/blob/8a26f10e77f2a1d1a56b35bbc29c60f4c2ab1ea2/manual/src/tricky_cases.md)).
5. **Caps.**
   - `DFT_BYTE_LIMIT` 1,000,000 and `DFT_GRAPH_LIMIT` 3,000,000 vertices. Past either it falls back to
     the line-oriented diff
     ([options.rs L17-L22, L336-L356](https://github.com/Wilfred/difftastic/blob/8a26f10e77f2a1d1a56b35bbc29c60f4c2ab1ea2/src/options.rs#L336-L356)).
   - In the text fallback, word diffing is skipped when a novel block has more than
     **`MAX_WORDS_IN_LINE` = 1000** words per side. The comment says "word-level diffing is merely nice
     to have"
     ([line_parser.rs L11, L169-L177](https://github.com/Wilfred/difftastic/blob/8a26f10e77f2a1d1a56b35bbc29c60f4c2ab1ea2/src/line_parser.rs#L169-L177)).

### VS Code diff editor ("advanced" algorithm, the default)

1. **Pairing.** There are no line pairs: it is a **whole-block** approach.
   - **Lines.** It hashes lines by their trimmed text. With fewer than 1,700 lines in total it uses a
     dynamic-programming LCS whose equality score is higher for longer equal lines (`1 + log(1 +
     len)`); otherwise it uses Myers
     ([defaultLinesDiffComputer.ts L60-L88](https://github.com/microsoft/vscode/blob/5042306fad013580e4565aa0c61efcac056af809/src/vs/editor/common/diff/defaultLinesDiffComputer/defaultLinesDiffComputer.ts#L60-L92)).
   - **Characters.** Each changed line range is then **re-diffed as one character sequence spanning all
     its lines**, with `\n` elements between lines (`refineDiff` →
     [`LinesSliceCharSequence` L14-L49](https://github.com/microsoft/vscode/blob/5042306fad013580e4565aa0c61efcac056af809/src/vs/editor/common/diff/defaultLinesDiffComputer/linesSliceCharSequence.ts#L14-L49)).
   - The results are "inner changes": `RangeMapping`s that can span lines. They map back through
     `translateOffset`/`translateRange`, a binary search over line start offsets that re-adds the
     trimmed indentation
     ([L101-L117](https://github.com/microsoft/vscode/blob/5042306fad013580e4565aa0c61efcac056af809/src/vs/editor/common/diff/defaultLinesDiffComputer/linesSliceCharSequence.ts#L101-L117)).
   - **Side-by-side alignment** is derived from inner changes ("inner hunk alignment"). Where an inner
     change starts after column 1 or ends before end of line, the view emits an alignment point, so
     those lines sit opposite each other
     ([diffEditorViewZones.ts L102, L580-L595](https://github.com/microsoft/vscode/blob/5042306fad013580e4565aa0c61efcac056af809/src/vs/editor/browser/widget/diffEditor/components/diffEditorViewZones/diffEditorViewZones.ts#L580-L595)).
2. **Granularity.** Characters, snapped to words afterwards (point 4). `ignoreTrimWhitespace` defaults to
   true and drops leading and trailing whitespace from the character sequence
   ([diffEditor.ts L14-L22](https://github.com/microsoft/vscode/blob/5042306fad013580e4565aa0c61efcac056af809/src/vs/editor/common/config/diffEditor.ts#L14-L22)).
3. **Inner algorithm.** DP LCS that "prefer[s] consecutive diagonals" when the slices total fewer than
   500 chars, else Myers
   ([L224-L226](https://github.com/microsoft/vscode/blob/5042306fad013580e4565aa0c61efcac056af809/src/vs/editor/common/diff/defaultLinesDiffComputer/defaultLinesDiffComputer.ts#L217-L263);
   [dynamicProgrammingDiffing.ts L45](https://github.com/microsoft/vscode/blob/5042306fad013580e4565aa0c61efcac056af809/src/vs/editor/common/diff/defaultLinesDiffComputer/algorithms/dynamicProgrammingDiffing.ts#L14-L106)).
4. **Noise heuristics.** These run in order, in
   [heuristicSequenceOptimizations.ts](https://github.com/microsoft/vscode/blob/5042306fad013580e4565aa0c61efcac056af809/src/vs/editor/common/diff/defaultLinesDiffComputer/heuristicSequenceOptimizations.ts).
   - `joinSequenceDiffsByShifting`, run twice, then `shiftSequenceDiffs`. These slide diffs to the
     best-scoring boundary, within 100 steps
     (L12-L20, L155-L200). Boundary scores: separator `,`/`;` = 30, an end = 10, a line break = 150 if
     it precedes the change, a category change +10
     ([linesSliceCharSequence.ts L71-L99, L207-L217](https://github.com/microsoft/vscode/blob/5042306fad013580e4565aa0c61efcac056af809/src/vs/editor/common/diff/defaultLinesDiffComputer/linesSliceCharSequence.ts#L71-L99)).
   - `extendDiffsToEntireWordIfAppropriate`: when less than 2/3 of a touched word is equal, the whole
     word is marked (L222-L299, rule at L280). An optional subword (camelCase) pass is behind
     `extendToSubwords`.
   - `removeShortMatches`: joins diffs separated by ≤ 2 equal chars (L203-L220).
   - `removeVeryShortMatchingTextBetweenLongDiffs`: joins diffs across an equal gap of ≤ 20 chars on one
     line when the surrounding diffs are "long". It uses a weighted formula and caps (L372-L431). It
     also absorbs ≤ 3-char line prefixes and suffixes into diffs longer than 100 chars (L440-L470).
   - On the line level, `removeVeryShortMatchingLinesBetweenDiffs` joins line diffs separated by ≤ 4
     non-whitespace chars (L325-L370).
   - **Legacy algorithm, for comparison.** It computed char changes only when both sides had fewer
     than 20 lines, over the whole block including `\n`. It then merged changes less than 3 chars apart
     ([legacyLinesDiffComputer.ts L15, L174-L197, L329-L358, L403](https://github.com/microsoft/vscode/blob/5042306fad013580e4565aa0c61efcac056af809/src/vs/editor/common/diff/legacyLinesDiffComputer.ts#L329-L358)).
5. **Caps.** `maxComputationTime` 5000 ms, after which the DP returns a trivial "timed out" diff, and
   `maxFileSize` 50 MB ([diffEditor.ts L14-L15](https://github.com/microsoft/vscode/blob/5042306fad013580e4565aa0c61efcac056af809/src/vs/editor/common/config/diffEditor.ts#L14-L15)).

### JetBrains IDEs (`ComparisonManagerImpl` / `ByWordRt`)

1. **Pairing.** **Whole-block word diff, then split back into line sub-blocks.** For each changed line
   fragment, `createInnerWordFragments` calls `ByWord.compareAndSplit` on the whole multi-line
   subsequence
   ([ComparisonManagerImpl.java L280-L313](https://github.com/JetBrains/intellij-community/blob/1ca4148fb98a4db8e51666f1d6bcd76b57fcf595/platform/diff-impl/src/com/intellij/diff/comparison/ComparisonManagerImpl.java#L280-L313)).
   `compareAndSplit` diffs words across the block, newlines included as chunks. `LineFragmentSplitter`
   then cuts the block at matched newlines, and at matched first-words of lines, into smaller line
   blocks. It merges neighbours that matched only on `\n`, differ only in whitespace, or contain no
   words
   ([ByWordRt.kt L89-L134](https://github.com/JetBrains/intellij-community/blob/1ca4148fb98a4db8e51666f1d6bcd76b57fcf595/platform/util/diff/src/com/intellij/diff/comparison/ByWordRt.kt#L89-L134);
   [LineFragmentSplitter.kt L29-L146](https://github.com/JetBrains/intellij-community/blob/1ca4148fb98a4db8e51666f1d6bcd76b57fcf595/platform/util/diff/src/com/intellij/diff/comparison/LineFragmentSplitter.kt#L29-L146)).
   The side-by-side view thus gets **finer line alignment derived from word matches**.
   - A registry flag, `diff.by.word.deprioritize.line.differences`, switches to `compareWordsFirst`. That
     diffs words over the whole file and lifts the result to line blocks. It is meant for changes such
     as Java→Kotlin or re-wrapped prose
     ([ByWordRt.kt L165-L235](https://github.com/JetBrains/intellij-community/blob/1ca4148fb98a4db8e51666f1d6bcd76b57fcf595/platform/util/diff/src/com/intellij/diff/comparison/ByWordRt.kt#L165-L235);
     [ComparisonManagerImpl.java L165-L168](https://github.com/JetBrains/intellij-community/blob/1ca4148fb98a4db8e51666f1d6bcd76b57fcf595/platform/diff-impl/src/com/intellij/diff/comparison/ComparisonManagerImpl.java#L158-L176)).
2. **Granularity.** Words come first and **ignore punctuation and whitespace**. `getInlineChunks` emits
   only word chunks (runs of non-whitespace, non-ASCII-punctuation code points), newline chunks, and one
   chunk per character for "continuous scripts" (CJK, Thai, …)
   ([ByWordRt.kt L638-L688](https://github.com/JetBrains/intellij-community/blob/1ca4148fb98a4db8e51666f1d6bcd76b57fcf595/platform/util/diff/src/com/intellij/diff/comparison/ByWordRt.kt#L638-L688);
   [TrimUtil.kt L24-L45](https://github.com/JetBrains/intellij-community/blob/1ca4148fb98a4db8e51666f1d6bcd76b57fcf595/platform/util/diff/src/com/intellij/diff/comparison/TrimUtil.kt#L24-L45)).
   Punctuation is matched **afterwards**, by `AdjustmentPunctuationMatcher`. It compares the punctuation
   between pairs of already-matched words character by character
   ([L695-L878](https://github.com/JetBrains/intellij-community/blob/1ca4148fb98a4db8e51666f1d6bcd76b57fcf595/platform/util/diff/src/com/intellij/diff/comparison/ByWordRt.kt#L695-L760)).
   Whitespace is corrected last, per comparison policy.
3. **Inner algorithm.** `DiffIterableUtil.diff` → `Diff.buildChanges`. It discards elements unique to
   one side (`Reindexer.discardUnique`) and runs Myers LCS with a threshold. If Myers exceeds the
   threshold, it falls back to patience
   ([Diff.kt L70-L100](https://github.com/JetBrains/intellij-community/blob/1ca4148fb98a4db8e51666f1d6bcd76b57fcf595/platform/util/diff/src/com/intellij/util/diff/Diff.kt#L70-L100);
   [MyersLCS.kt L75-L80](https://github.com/JetBrains/intellij-community/blob/1ca4148fb98a4db8e51666f1d6bcd76b57fcf595/platform/util/diff/src/com/intellij/util/diff/MyersLCS.kt#L75-L80)).
4. **Noise heuristics.** `WordChunkOptimizer` shifts ambiguous word changes so that they are separated
   by whitespace (`[X]A Y[A ZA] -> [XA] YA [ZA]`)
   ([ChunkOptimizer.kt L100-L170](https://github.com/JetBrains/intellij-community/blob/1ca4148fb98a4db8e51666f1d6bcd76b57fcf595/platform/util/diff/src/com/intellij/diff/comparison/ChunkOptimizer.kt#L100-L170)).
   Punctuation-second matching stops `,` `{` `(` from anchoring alignments.
5. **Caps.**
   - The Myers threshold is `max(20000 + 10·√(n1+n2), DELTA_THRESHOLD_SIZE = 20000)`.
   - After **`MAX_BAD_LINES` = 3** fragments throw `DiffTooBigException`, inner fragments are no longer
     attempted for that comparison
     ([DiffConfig.kt L7-L13](https://github.com/JetBrains/intellij-community/blob/1ca4148fb98a4db8e51666f1d6bcd76b57fcf595/platform/util/diff/src/com/intellij/util/diff/DiffConfig.kt#L7-L13);
     [ComparisonManagerImpl.java L225-L240](https://github.com/JetBrains/intellij-community/blob/1ca4148fb98a4db8e51666f1d6bcd76b57fcf595/platform/diff-impl/src/com/intellij/diff/comparison/ComparisonManagerImpl.java#L218-L241)).

### Gerrit (`IntraLineLoader`)

1. **Pairing.** **Whole-block.** For every `REPLACE` edit, a `CharText` is built over the whole run of
   replaced lines, newlines kept (`/* keep LF */`). The two texts are diffed as single character
   sequences
   ([IntraLineLoader.java L108-L125](https://github.com/GerritCodeReview/gerrit/blob/1820128f06cf741cd11e7a78a47f665000f4734f/java/com/google/gerrit/server/patch/IntraLineLoader.java#L108-L125);
   [CharText.java L23](https://github.com/GerritCodeReview/gerrit/blob/1820128f06cf741cd11e7a78a47f665000f4734f/java/com/google/gerrit/server/patch/CharText.java#L23)).
   - The REST API returns `edit_a`/`edit_b` as `<skip, mark>` lengths relative to the start of the chunk.
     "It is possible for the edits to span newlines"
     ([rest-api-changes.txt L8688-L8701](https://github.com/GerritCodeReview/gerrit/blob/1820128f06cf741cd11e7a78a47f665000f4734f/Documentation/rest-api-changes.txt#L8688-L8701)).
   - The web UI maps them back to per-line highlights by walking the line lengths (+1 for `\n`)
     ([gr-diff-processor.ts `convertIntralineInfos` L591-L641](https://github.com/GerritCodeReview/gerrit/blob/1820128f06cf741cd11e7a78a47f665000f4734f/polygerrit-ui/app/embed/diff/gr-diff-processor/gr-diff-processor.ts#L591-L641)).
   - The side-by-side then places rows **positionally**: `removes[i]` opposite `adds[i]`, padding the
     shorter side
     ([gr-diff-group.ts L432-L454](https://github.com/GerritCodeReview/gerrit/blob/1820128f06cf741cd11e7a78a47f665000f4734f/polygerrit-ui/app/embed/diff/gr-diff/gr-diff-group.ts#L432-L454)).
   - **This is exactly the combination WispyDiff would get from recommendation 1:** block-level
     highlighting with positional rows.
2. **Granularity.** Characters.
3. **Inner algorithm.** JGit `MyersDiff` over the char text
   ([L124](https://github.com/GerritCodeReview/gerrit/blob/1820128f06cf741cd11e7a78a47f665000f4734f/java/com/google/gerrit/server/patch/IntraLineLoader.java#L124)).
   The release notes call it "largely driven by a simple Myers O(ND) character difference over the
   replaced lines" ([Gerrit 2.1.2 release notes](https://opendev.org/opendev/gerrit/commit/2b4eef576bcfe88ed100d59a42d0870367e332f4)).
4. **Noise heuristics.** These are in
   [L128-L262](https://github.com/GerritCodeReview/gerrit/blob/1820128f06cf741cd11e7a78a47f665000f4734f/java/com/google/gerrit/server/patch/IntraLineLoader.java#L128-L262).
   - Edits **≤ 5 chars apart** are joined, unless the gap contains a newline (`canCoalesce`).
   - An INSERT/DELETE next to a REPLACE is merged into it.
   - Identical edges are trimmed.
   - Edits are slid to prefer the tail.
   - A nearly-whole-line edit absorbs the line's LF.
   - **Before** the char diff, `combineLineEdits` merges line edits separated by one "pointless" line:
     blank, `{`, `}`, `/**`, `*`, or a line ending in `{`/`:`. This keeps re-indented blocks together
     ([L50-L53, L352-L398](https://github.com/GerritCodeReview/gerrit/blob/1820128f06cf741cd11e7a78a47f665000f4734f/java/com/google/gerrit/server/patch/IntraLineLoader.java#L352-L398)).
   - It validates that applying the edits reproduces B. Otherwise it falls back to one whole-region edit
     ([L258-L262](https://github.com/GerritCodeReview/gerrit/blob/1820128f06cf741cd11e7a78a47f665000f4734f/java/com/google/gerrit/server/patch/IntraLineLoader.java#L256-L262)).
5. **Caps.** A `cache.diff_intraline.timeout` of 5 s. On timeout, intraline is disabled for that file
   pair ([L66-L74](https://github.com/GerritCodeReview/gerrit/blob/1820128f06cf741cd11e7a78a47f665000f4734f/java/com/google/gerrit/server/patch/IntraLineLoader.java#L66-L74);
   [config-gerrit.txt L1595-L1604](https://github.com/GerritCodeReview/gerrit/blob/1820128f06cf741cd11e7a78a47f665000f4734f/Documentation/config-gerrit.txt#L1595-L1604)).
   Results are cached server-side (`IntraLineDiffKey`).

### Phabricator / Differential (with Arcanist utilities)

1. **Pairing.** Positional. `reparseHunksForSpecialAttributes` pads old and new line arrays with `null`
   so they have equal length
   ([DifferentialHunkParser.php L200-L268](https://github.com/phacility/phabricator/blob/5720a38cfe95b00ca4be5016dd0d2f3195f4fa04/src/applications/differential/parser/DifferentialHunkParser.php#L200-L268)).
   `generateIntraLineDiffs` then compares `$old[$key]` with `$new[$key]` whenever their types differ
   ([L270-L353](https://github.com/phacility/phabricator/blob/5720a38cfe95b00ca4be5016dd0d2f3195f4fa04/src/applications/differential/parser/DifferentialHunkParser.php#L270-L353)).
   An indentation-only change is detected first and shown as a depth marker.
2. **Granularity.** Characters: bytes, or UTF-8 glyphs when non-ASCII
   ([ArcanistDiffUtils.php L107-L117](https://github.com/phacility/arcanist/blob/e50d1bc4eabac9c37e3220e9f3fb8e37ae20b957/src/difference/ArcanistDiffUtils.php#L107-L117)).
3. **Inner algorithm.** `PhutilEditDistanceMatrix`, a full Levenshtein DP. It strips the common prefix
   and suffix first, uses replace cost 2, and adds a tiny **alter cost** (`1/(max·2)`) to switch
   operation type. That favours long runs of one operation
   ([ArcanistDiffUtils.php L96-L105](https://github.com/phacility/arcanist/blob/e50d1bc4eabac9c37e3220e9f3fb8e37ae20b957/src/difference/ArcanistDiffUtils.php#L96-L105);
   [PhutilEditDistanceMatrix.php L1-L46, L148-L182](https://github.com/phacility/arcanist/blob/e50d1bc4eabac9c37e3220e9f3fb8e37ae20b957/src/utils/PhutilEditDistanceMatrix.php#L1-L46)).
4. **Noise heuristics.** "Internal smoothing" repeatedly turns equal runs of 1–3 chars between edits into
   replacements. Per the source, this gives "fewer choppy runs of short added and removed substrings"
   ([PhutilEditDistanceMatrix.php L540-L561](https://github.com/phacility/arcanist/blob/e50d1bc4eabac9c37e3220e9f3fb8e37ae20b957/src/utils/PhutilEditDistanceMatrix.php#L540-L561)).
5. **Caps.** It works on at most **100 glyphs** after prefix/suffix trimming. Over that, the middle is
   all delete + insert ([L252-L257](https://github.com/phacility/arcanist/blob/e50d1bc4eabac9c37e3220e9f3fb8e37ae20b957/src/utils/PhutilEditDistanceMatrix.php#L252-L257)).
   Inputs whose combined length exceeds 1,600 bytes are marked changed **whole-line** without diffing
   ([ArcanistDiffUtils.php L59-L81](https://github.com/phacility/arcanist/blob/e50d1bc4eabac9c37e3220e9f3fb8e37ae20b957/src/difference/ArcanistDiffUtils.php#L59-L81)).
   Phabricator therefore fails "loud" (everything emphasised), where we fail "quiet" (nothing
   emphasised).

### Meld

1. **Pairing.** None, because both files are shown whole, with connector curves. For each `replace`
   chunk, the **whole chunk text, newlines included**, is diffed
   ([filediff.py L1989-L2008](https://github.com/GNOME/meld/blob/83178360e552417510f876669427dc3c50b558b6/meld/filediff.py#L1989-L2008)).
   The work is cached and runs in a worker process
   ([matchers/helpers.py L14-L45](https://github.com/GNOME/meld/blob/83178360e552417510f876669427dc3c50b558b6/meld/matchers/helpers.py#L14-L45)).
2. **Granularity.** Characters.
3. **Inner algorithm.** `InlineMyersSequenceMatcher`. Before Myers, it discards characters not part of
   any 3-gram common to both sides, when that removes more than 10
   ([matchers/myers.py L338-L373](https://github.com/GNOME/meld/blob/83178360e552417510f876669427dc3c50b558b6/meld/matchers/myers.py#L338-L373)).
4. **Noise heuristics.** Equal matches shorter than 3 chars are dropped, except at the start or end of
   the chunk ([filediff.py L2048-L2062](https://github.com/GNOME/meld/blob/83178360e552417510f876669427dc3c50b558b6/meld/filediff.py#L2048-L2062)).
   A zero-width combining diacritic is widened to its character.
5. **Caps.** If `len(text1)+len(text2) > 20000`, the whole chunk is tagged and the user is offered to
   force highlighting ([L2006-L2012](https://github.com/GNOME/meld/blob/83178360e552417510f876669427dc3c50b558b6/meld/filediff.py#L2006-L2012)).

---

## Comparison table

| Tool | Pairing | Tokens | Inner algorithm | Noise suppression | Caps | Cross-line? |
|---|---|---|---|---|---|---|
| diff-highlight / diff-so-fancy | positional, equal counts only | chars | common prefix + suffix, one span | skip if whole line would light up | none | no |
| git `--word-diff` | none (whole run) | non-space runs / regex, per-language drivers | xdiff Myers over word-per-line | none | none | **yes** |
| delta | **similarity**, greedy look-ahead, distance ≤ 0.6 | `\w+` + single graphemes | Wagner–Fischer, run-open penalty | distance gate, whitespace coalescing | line buffer 32, 3000-char lines | no |
| diff-match-patch | n/a | chars (line/word modes via mapping) | Myers bisect + half-match | **semantic cleanup**, boundary sliding | 1 s timeout | n/a |
| GitLab | positional, balanced `-…+` runs only | chars | dmp + `diff_cleanupSemantic` | dmp semantic; skip empty old line | dmp 1 s | no |
| GitHub | **unverified** | **unverified** | **unverified** | **unverified** | diff-level limits only | **unverified** |
| difftastic | tree diff; display aligns by matched tokens | syntax nodes; words in strings/comments; words in text fallback | Dijkstra (tree), Histogram (linear) | punctuation cost, `has_common_words` | 1 MB, 3M vertices, 1000 words/block | **yes** (text fallback) |
| VS Code | none; side-by-side aligned from inner changes | chars snapped to words/subwords | DP (<500 chars) else Myers | shifting to boundaries, word extension, join ≤2-char gaps, long-diff joining | 5 s, 50 MB | **yes** |
| JetBrains | none; block split into line blocks from word matches | words (punctuation second, CJK per char) | Myers w/ threshold → patience | chunk shifting to whitespace, line-block merging | Myers threshold, 3 bad fragments | **yes** |
| Gerrit | positional rows; diff is per block | chars | JGit Myers | join ≤5-char gaps, edge trimming, pointless-line merging, validation | 5 s timeout | **yes** |
| Phabricator | positional | chars/glyphs | Levenshtein + alter cost | smoothing of ≤3-char equal runs | 100 glyphs; 1600 bytes → whole line | no |
| Meld | none (whole panes) | chars | Myers + 3-gram prefilter | drop equal runs <3 | 20,000 chars | **yes** |
| **WispyDiff (now)** | positional, i-th with i-th | words, whitespace runs, single punctuation | prefix/suffix trim + LCS table | whitespace bridging, 60% changed gate | 40k cells/pair | no |

---

## Whole-block (cross-line) approaches and mapping back to lines

Six of the tools diff a change block as one text: git `--word-diff`, VS Code, JetBrains, Gerrit, Meld, and
difftastic's text fallback. They share one recipe:

1. Concatenate the block's deleted lines into A and its added lines into B, keeping `\n` as an element
   or token. Gerrit and Meld keep LF in the text. VS Code pushes `'\n'` elements. JetBrains emits
   `NewlineChunk`s. difftastic joins the lines with their newlines. git appends the raw lines.
2. Diff A and B as single sequences. Tokens on different lines can match, so a joined or split line
   matches its pieces.
3. Map back. Keep each line's start offset (VS Code `firstElementOffsetByLineIdx` + `translateOffset`;
   difftastic `LinePositions::from_region`), or walk the line lengths (Gerrit `convertIntralineInfos`).
   A changed range that crosses a newline is cut into one span per line. The newline itself is never
   drawn.
4. Optionally, derive the side-by-side alignment from the result:
   - VS Code: alignment points where an inner change leaves text before or after it on a line.
   - JetBrains: `LineFragmentSplitter` cuts at matched newlines and matched first words.
   - difftastic: lines placed opposite the lines holding their matched tokens.

   Gerrit shows the alternative: keep positional rows and accept that a token's match may sit on a
   different row.

This explains our weak case. The new line `return { orderId, warehouseId, sendMail: false };` is
assembled from several deleted lines. No choice of *one* opposite line can explain it, and only a
block-level diff can.

**Scratch simulation.** This is an illustrative reconstruction of the weak case, not taken from a real
PR. It was run with a throwaway Python script that reimplements our tokenizer and LCS. Nothing in the
repo was built or run.

```
old:  return {                              new:  const orderId = order.id;
          orderId: order.id,                      const warehouseId = order.warehouseId;
          warehouseId: order.warehouseId,
          sendMail: false,                        return { orderId, warehouseId, sendMail: false };
      };
```

- **Today, positional pairs.** `sendMail: false,` sits opposite the `return` line, with a changed ratio
  of 0.52, under the 0.6 gate. On the new line, `return`, `{`, `orderId`, `,`, `warehouseId`, `,`, `}`,
  `;` are highlighted. Two other pairs are 100% changed and suppressed.
- **Whole block, plain LCS.** The `const` lines now only mark `const`, `=`, `;`. The `return` line
  still marks `return {`, `orderId`, `warehouseId,`. LCS is order-preserving: `orderId` and
  `warehouseId` match their first occurrences, on the `const` lines, and the old `return {` can't match
  after them.
- **Whole block, unique-token anchoring** (patience-style: anchor on tokens occurring once on each side,
  LCS in between). The `return` line has **no highlights**. The `const` lines are fully changed. The old
  side marks only `: order.id`, `: order.`, and the trailing `,`. This is the "right" answer.
  - Histogram diff, which `imara-diff` provides and difftastic uses, is a variant of patience
    ([imara-diff README](https://github.com/pascalkuthe/imara-diff)).
  - **Unverified:** I did not check that its exact output on this input matches the simulation.

---

## Recommendations for WispyDiff (ranked)

### 1. Diff each change block as one token stream, anchored on rare tokens; map back to per-line spans

- **What.** In `model.rs::mark_words`, for each `dels`/`adds` run:
  - Tokenize all deleted lines into one sequence, with an explicit newline token between lines. Do the
    same for the added lines.
  - Record `(row index, utf16 start, utf16 end)` for every token.
  - Run a histogram or patience diff over interned tokens. `imara-diff`'s `Algorithm::Histogram` is what
    difftastic uses (lcs_diff.rs).
  - Turn changed tokens into per-row spans. Never emit newline tokens, and never bridge across them.
  - The output stays `Vec<(u32, u32)>` per row, so the frontend and the cache format don't change.
- **What it fixes.**
  - The weak case (simulation above).
  - Any join, split, or re-wrap of lines: object literals collapsed to one line, argument lists
    exploded, chained calls re-broken, prettier/php-cs-fixer reflows.
  - The diff-highlight README's "line removed at top, added at bottom" misalignment, because pairing no
    longer matters.
  - Unequal blocks. Today an extra added line shifts every later pair.
- **Cost at our scale.**
  - ~50k changed lines × ~10 tokens is ~0.5M tokens per side in the worst case, spread over blocks.
    Histogram/Myers on interned tokens is near-linear for similar texts.
  - imara-diff's README says its Myers has "heuristics to ensure fast runtime in pathological cases to
    avoid quadratic time complexity", and that Histogram beats its Myers by 10–100%.
  - A full LCS table over a block is **not** viable: a 300-line block at ~10 tokens/line is 9M cells. It
    must be Myers or Histogram.
  - Keep a per-block cap, like difftastic's 1,000 words per block or Meld's 20,000 chars, and use the
    current pairwise path above it. Starting point: ~2,000–5,000 tokens per side.
  - Run per file with `rayon`, which is already a dependency. It is precomputed per SHA and cached, so
    it adds to ingest time only.
  - *Estimate, not measured.* Verify with `just bench`. It touches the core.
- **Interaction with positional side-by-side.**
  - Highlights become independent of pairing. That is fine: Gerrit ships exactly this, with block-level
    intraline and `removes[i]` opposite `adds[i]`.
  - The cost is that an unhighlighted token on the left may have its match on a different right-hand
    row, e.g. old `return {` opposite new `const orderId …`. That is still more truthful than
    highlighting tokens that do exist in the block.
  - Unified view benefits most, because it has no pairing at all.
  - The doc comment "pairs each change block's deleted lines with its added lines in order (as the
    side-by-side puts them opposite each other)" stops being true. `split::align` is untouched.

### 2. Semantic cleanup of the token edit script

- **What.** Run these after the diff, all O(tokens).
  - **(a) dmp chaff rule.** Turn an unchanged run whose non-whitespace length is ≤ the changed length on
    *both* sides into a change (`diff_cleanupSemantic`, L788-L812). Repeat until stable.
  - **(b) Absorb short punctuation-only gaps.** Absorb a gap of ≤ 2–3 chars, or a single
    non-word token, between two changes on the same line. Gerrit uses ≤5, Meld <3, VS Code ≤2, and
    Phabricator smooths ≤3.
  - **(c) Keep the existing whitespace bridging.** delta does the same.
  - **(d) Optional, JetBrains-style.** Diff word tokens first, *excluding* punctuation and whitespace.
    Then match punctuation only between already-matched words. This stops `,` `{` `(` `;` from anchoring
    alignments.
- **What it fixes.** Choppy "confetti" highlights like `[a]x[b]`, the diff-highlight README's
  `-{i}nt-{ere}sti-{ng}`. This matters more after recommendation 1, because larger sequences produce
  more coincidental single-token matches: `,`, `{`, `return`, `this`, `$`. Rule (d) directly targets the
  failure mode where punctuation steals a match from an identifier (difftastic charges a punctuation
  penalty for the same reason, graph.rs L341-L355).
- **Cost.** Negligible: linear passes over the edit script.
- **Interaction.** None with pairing. Works for both the block path and the pairwise fallback.

### 3. Replace the per-pair 60% gate with a block gate plus a "whole line" rule

- **What.**
  - **(a) Block gate.** Suppress all emphasis in a block when it is mostly rewritten. Either use our
    weighted ratio over the block, or difftastic's `unchanged > 2 && unchanged*2 >= novel` (syntax.rs
    L849).
  - **(b) Whole-line rule.** A row whose non-whitespace text is entirely changed gets **no** spans. The
    added/deleted background already says "this line is new". This is diff-highlight's "don't highlight
    if the whole line would be" (L303-L325).
  - **(c) Optional per-line gate.** Keep a per-line ratio, on that line's own tokens, as a guard against
    noisy lines.
  - Note: our current ratio is delta's distance metric (changed / (changed + 2·unchanged)), and 0.6 is
    delta's default. The value is reasonable. What doesn't carry over to block mode is the *unit*: a pair.
- **What it fixes.**
  - In block mode, genuinely new lines inside a block (the two `const` lines) render as plain added
    lines instead of being fully emphasised.
  - Real rewrites stay quiet, as today.
  - It avoids the Phabricator failure mode of "loud" whole-line emphasis.
- **Cost.** Negligible.
- **Interaction.** None.

### 4. Keep the pairwise path as the fallback; optionally pair by similarity there (delta-style)

- **What.**
  - For blocks over the token cap, or if recommendation 1 is deferred, keep `diff_words` as it is.
  - Optionally, pair by similarity instead of by index: for each deleted line, look ahead for the first
    added line with distance ≤ 0.6, as in delta (edits.rs L46-L103). Bound the look-ahead, e.g. to 8
    lines, to stay O(n).
- **What it fixes.** Insertions or deletions at the top of a block that shift every later pair. It
  doesn't fix N:1 restructurings like the weak case.
- **Cost.** Up to look-ahead × per-pair LCS. Bounded by the window and the existing 40k-cell cap.
- **Interaction.** With *highlighting*-only pairing, the highlighted partner may no longer be the line
  opposite in side-by-side. That is acceptable, but less intuitive than recommendation 1. Changing
  `split::align` to the same pairs would add filler rows inside blocks. That is a UI behaviour change,
  so check SPEC.md first.

### 5. Later, optional: derive side-by-side alignment from the block diff (VS Code / JetBrains / difftastic)

- **What.** Use the matches from recommendation 1 to put lines that share matched tokens opposite each
  other:
  - VS Code emits an alignment point where an inner change leaves text before or after it on a line.
  - JetBrains' `LineFragmentSplitter` splits at matched newlines and first words.
  - Insert filler rows elsewhere.
- **What it fixes.** In side-by-side, `return {` … `};` would sit opposite the new `return` line, with
  the `const` lines opposite fillers. It is the most readable result.
- **Cost.** Cheap to compute, but it changes `split::align`, block navigation (`blocks`), and the e2e
  expectations. It is a UX decision for SPEC.md, not a highlighting fix.
- **Interaction.** It replaces positional pairing. Keep positional pairing for blocks without block-diff
  results (over the cap).

### Not recommended now

- **Structural (tree-sitter) diffing à la difftastic.** It is a large project with its own blow-up
  limits: 3M graph vertices, then a line-diff fallback. The highlighting problem we have is solved by
  block-level token diffing.
- **A small reuse.** We already parse with tree-sitter for highlighting. Its token boundaries could
  refine tokenization later, for example to keep string-literal contents word-diffed (difftastic's
  `split_atom_words`).
- **Character-level diffs (GitLab, Gerrit, VS Code).** They need extra machinery to stay readable: word
  snapping, boundary scores, gap joining. Tokens give us most of that for free.
