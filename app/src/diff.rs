//! The part of a list that changed, so a list model is spliced there only
//! (a list view keeps its other rows, and its scroll position).

/// What of `old` becomes `new`: `(start, removed, added)`, the range of
/// `old` that `added` items of `new` replace; `same` items before and
/// after it stay.
pub fn changed_range<T>(
    old: &[T],
    new: &[T],
    same: impl Fn(&T, &T) -> bool,
) -> (usize, usize, usize) {
    let start = old.iter().zip(new).take_while(|(o, n)| same(o, n)).count();
    let end = old[start..]
        .iter()
        .rev()
        .zip(new[start..].iter().rev())
        .take_while(|(o, n)| same(o, n))
        .count();
    (start, old.len() - start - end, new.len() - start - end)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn range(old: &str, new: &str) -> (usize, usize, usize) {
        let (old, new): (Vec<char>, Vec<char>) = (old.chars().collect(), new.chars().collect());
        changed_range(&old, &new, |a, b| a == b)
    }

    /// Only what changed is replaced: a song removed, added or moved leaves
    /// the others' rows (and the scroll position) alone.
    #[test]
    fn only_the_changed_part_is_replaced() {
        assert_eq!(range("abcde", "abcde"), (5, 0, 0));
        assert_eq!(range("abcde", "abde"), (2, 1, 0));
        assert_eq!(range("abcde", "abXcde"), (2, 0, 1));
        assert_eq!(range("abcde", "adbce"), (1, 3, 3));
        assert_eq!(range("abcde", "bcde"), (0, 1, 0));
        assert_eq!(range("abcde", "abcd"), (4, 1, 0));
        assert_eq!(range("", "abc"), (0, 0, 3));
        assert_eq!(range("abc", ""), (0, 3, 0));
        // Repeated songs: start and end don't overlap.
        assert_eq!(range("aaa", "aa"), (2, 1, 0));
        assert_eq!(range("aa", "aaa"), (2, 0, 1));
        assert_eq!(range("abcde", "XYZ"), (0, 5, 3));
    }
}
