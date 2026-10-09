//! Full-file side-by-side: the whole base file next to the whole head file, aligned so each
//! change sits opposite what it replaced (with filler rows where one side has more lines).

use serde::{Deserialize, Serialize};

use crate::highlight::Seg;
use crate::model::{row_kind, Row};
use crate::words::Spans;

/// A side with no line opposite the other side's change.
pub const FILLER: u8 = 6;

/// One aligned pair of lines. Kinds use [`row_kind`] (`CONTEXT`, `DELETED`/`ADDED`) or [`FILLER`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Pair {
    pub o: Option<u32>,
    pub n: Option<u32>,
    pub ok: u8,
    pub nk: u8,
}

/// A rendered side-by-side row. Short field names: rows cross the IPC bridge in bulk.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SplitRow {
    pub o: Option<u32>,
    pub n: Option<u32>,
    pub ok: u8,
    pub nk: u8,
    pub os: Vec<Seg>,
    pub ns: Vec<Seg>,
    /// Stack index of the PR that last touched the old / new line (changed sides only).
    pub oa: Option<u8>,
    pub na: Option<u8>,
    /// The old / new line's number in that PR's own diff (changed sides only).
    pub ol: Option<u32>,
    pub nl: Option<u32>,
    /// The changed words of the old / new line (see [`crate::words`]).
    pub ow: Spans,
    pub nw: Spans,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Alignment {
    pub pairs: Vec<Pair>,
    /// Index into `pairs` where each change block starts, for `j`/`k` navigation.
    pub blocks: Vec<u32>,
}

/// Aligns the full old (`old_len` lines) and new (`new_len` lines) file around the changes in
/// `rows` (one file's unified rows: hunk headers and lines; other rows are ignored).
pub fn align(rows: &[Row], old_len: u32, new_len: u32) -> Alignment {
    let mut out = Alignment::default();
    let (mut o, mut n) = (1u32, 1u32);
    let mut dels: Vec<u32> = Vec::new();
    let mut adds: Vec<u32> = Vec::new();

    let flush = |out: &mut Alignment, dels: &mut Vec<u32>, adds: &mut Vec<u32>| {
        if dels.is_empty() && adds.is_empty() {
            return;
        }
        out.blocks.push(out.pairs.len() as u32);
        for i in 0..dels.len().max(adds.len()) {
            let (d, a) = (dels.get(i).copied(), adds.get(i).copied());
            out.pairs.push(Pair {
                o: d,
                n: a,
                ok: if d.is_some() { row_kind::DELETED } else { FILLER },
                nk: if a.is_some() { row_kind::ADDED } else { FILLER },
            });
        }
        dels.clear();
        adds.clear();
    };

    let mut at_hunk_start = false;
    for row in rows {
        match row.k {
            row_kind::HUNK => {
                flush(&mut out, &mut dels, &mut adds);
                at_hunk_start = true;
                continue;
            }
            row_kind::CONTEXT | row_kind::ADDED | row_kind::DELETED => {}
            _ => continue,
        }
        if at_hunk_start {
            // Unchanged lines between the previous hunk and this one, in lockstep.
            // Context and deleted rows carry an old line number; added rows only a new one.
            let behind = |o: u32, n: u32| match row.o {
                Some(ro) => o < ro,
                None => row.n.is_some_and(|rn| n < rn),
            };
            while behind(o, n) && (o <= old_len || n <= new_len) {
                out.pairs.push(Pair { o: Some(o), n: Some(n), ok: row_kind::CONTEXT, nk: row_kind::CONTEXT });
                o += 1;
                n += 1;
            }
            at_hunk_start = false;
        }
        match row.k {
            row_kind::DELETED => {
                if !adds.is_empty() {
                    // A deletion after additions starts a new block.
                    flush(&mut out, &mut dels, &mut adds);
                }
                dels.extend(row.o);
            }
            row_kind::ADDED => adds.extend(row.n),
            _ => {
                flush(&mut out, &mut dels, &mut adds);
                out.pairs.push(Pair { o: row.o, n: row.n, ok: row_kind::CONTEXT, nk: row_kind::CONTEXT });
            }
        }
        if let Some(ro) = row.o {
            o = ro + 1;
        }
        if let Some(rn) = row.n {
            n = rn + 1;
        }
    }
    flush(&mut out, &mut dels, &mut adds);
    while o <= old_len || n <= new_len {
        let (po, pn) = ((o <= old_len).then_some(o), (n <= new_len).then_some(n));
        out.pairs.push(Pair {
            o: po,
            n: pn,
            ok: if po.is_some() { row_kind::CONTEXT } else { FILLER },
            nk: if pn.is_some() { row_kind::CONTEXT } else { FILLER },
        });
        o += 1;
        n += 1;
    }
    out
}

/// Renders aligned pairs with highlighted full-file lines, taking attribution from the
/// file's unified rows.
pub fn split_rows(rows: &[Row], pairs: &[Pair], old: &[Vec<Seg>], new: &[Vec<Seg>]) -> Vec<SplitRow> {
    use std::collections::HashMap;
    let mut old_attr: HashMap<u32, (u8, Option<u32>)> = HashMap::new();
    let mut new_attr: HashMap<u32, (u8, Option<u32>)> = HashMap::new();
    let mut old_text: HashMap<u32, &Vec<Seg>> = HashMap::new();
    let mut new_text: HashMap<u32, &Vec<Seg>> = HashMap::new();
    let mut old_words: HashMap<u32, &Spans> = HashMap::new();
    let mut new_words: HashMap<u32, &Spans> = HashMap::new();
    for row in rows {
        match row.k {
            row_kind::DELETED => {
                if let (Some(o), Some(a)) = (row.o, row.a) {
                    old_attr.insert(o, (a, row.l));
                }
                if let Some(o) = row.o {
                    old_text.insert(o, &row.s);
                    old_words.insert(o, &row.w);
                }
            }
            row_kind::ADDED => {
                if let (Some(n), Some(a)) = (row.n, row.a) {
                    new_attr.insert(n, (a, row.l));
                }
                if let Some(n) = row.n {
                    new_text.insert(n, &row.s);
                    new_words.insert(n, &row.w);
                }
            }
            _ => {}
        }
    }
    // Prefer the full-file highlighting; fall back to the unified row's segments.
    let line = |lines: &[Vec<Seg>], fallback: &HashMap<u32, &Vec<Seg>>, no: Option<u32>| -> Vec<Seg> {
        let Some(no) = no else { return Vec::new() };
        lines
            .get(no as usize - 1)
            .or_else(|| fallback.get(&no).copied())
            .cloned()
            .unwrap_or_default()
    };
    let words = |words: &HashMap<u32, &Spans>, no: Option<u32>, changed: bool| -> Spans {
        no.filter(|_| changed).and_then(|no| words.get(&no)).map(|w| (*w).clone()).unwrap_or_default()
    };
    pairs
        .iter()
        .map(|p| {
            let old_pr = p.o.and_then(|o| old_attr.get(&o).copied()).filter(|_| p.ok == row_kind::DELETED);
            let new_pr = p.n.and_then(|n| new_attr.get(&n).copied()).filter(|_| p.nk == row_kind::ADDED);
            (p, old_pr, new_pr)
        })
        .map(|(p, old_pr, new_pr)| SplitRow {
            o: p.o,
            n: p.n,
            ok: p.ok,
            nk: p.nk,
            os: line(old, &old_text, p.o),
            ns: line(new, &new_text, p.n),
            oa: old_pr.map(|a| a.0),
            na: new_pr.map(|a| a.0),
            ol: old_pr.and_then(|a| a.1),
            nl: new_pr.and_then(|a| a.1),
            ow: words(&old_words, p.o, p.ok == row_kind::DELETED),
            nw: words(&new_words, p.n, p.nk == row_kind::ADDED),
        })
        .collect()
}
