//! The guides drawn before each PR of an inbox group whose stack branches: `├─`/`└─` where a
//! branch starts, `│` beside the PRs of a branch while a later one is still to come.

/// One prefix per PR of a group listed depth-first (`parent(i)`: the PR it builds on, listed
/// before it). A plain stack gets empty ones.
pub fn guides(len: usize, parent: impl Fn(usize) -> Option<usize>) -> Vec<String> {
    let mut out: Vec<String> = Vec::with_capacity(len);
    for i in 0..len {
        let Some(up) = parent(i).filter(|&p| p < i) else {
            out.push(String::new());
            continue;
        };
        // The parent's columns go on: a branch start becomes the line beside its branch.
        let mut prefix = out[up].replace("├─ ", "│  ").replace("└─ ", "   ");
        let siblings: Vec<usize> = (0..len).filter(|&c| parent(c) == Some(up)).collect();
        if siblings.len() > 1 {
            prefix.push_str(if siblings.last() == Some(&i) { "└─ " } else { "├─ " });
        }
        out.push(prefix);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::guides;

    fn of(parents: &[Option<usize>]) -> Vec<String> {
        guides(parents.len(), |i| parents[i])
    }

    #[test]
    fn a_plain_stack_has_no_guides() {
        assert_eq!(of(&[None, Some(0), Some(1)]), vec!["", "", ""]);
    }

    #[test]
    fn branches_get_a_corner_and_a_line_beside_all_but_the_last() {
        // #1 ← {#2 ← #4, #3}
        assert_eq!(of(&[None, Some(0), Some(1), Some(0)]), vec!["", "├─ ", "│  ", "└─ "]);
    }

    #[test]
    fn nested_branches_keep_the_outer_line() {
        // #1 ← {#2 ← {#5, #6}, #3 ← #4}
        let parents = [None, Some(0), Some(1), Some(1), Some(0), Some(4)];
        assert_eq!(of(&parents), vec!["", "├─ ", "│  ├─ ", "│  └─ ", "└─ ", "   "]);
    }
}
