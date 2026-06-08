//! Content fingerprinting and edit-distance utilities.
//!
//! Two jobs:
//!   * [`fingerprint`] gives a cheap, whitespace-insensitive identity for a
//!     file body so we can tell "the agent put the file back exactly how it
//!     was" from "the agent made a real change".
//!   * [`similarity`] (Levenshtein-based) catches *near*-reverts: cosmetically
//!     different edits that are semantically the same churn.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

/// Stable 64-bit content fingerprint over the whitespace-normalised body.
/// Reformatting alone (re-indentation, blank-line churn) does not change it.
pub fn fingerprint(content: &str) -> u64 {
    let mut h = DefaultHasher::new();
    normalize(content).hash(&mut h);
    h.finish()
}

/// Collapse insignificant whitespace: tokenise on whitespace, rejoin with
/// single spaces. Keeps token *order* (so logic changes still register) while
/// ignoring layout.
pub fn normalize(content: &str) -> String {
    content.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Levenshtein edit distance between two strings.
///
/// Classic dynamic-programming formulation, O(n·m) time and O(min(n,m)) space.
pub fn levenshtein(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    if a.is_empty() {
        return b.len();
    }
    if b.is_empty() {
        return a.len();
    }
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    let mut curr: Vec<usize> = vec![0; b.len() + 1];
    for (i, &ca) in a.iter().enumerate() {
        curr[0] = i + 1;
        for (j, &cb) in b.iter().enumerate() {
            let cost = if ca == cb { 0 } else { 1 };
            curr[j + 1] = (prev[j + 1] + 1).min(curr[j] + 1).min(prev[j] + cost);
        }
        std::mem::swap(&mut prev, &mut curr);
    }
    prev[b.len()]
}

/// Similarity ratio in `[0.0, 1.0]` over the whitespace-normalised forms.
/// `1.0` means identical; `0.0` means maximally different.
pub fn similarity(a: &str, b: &str) -> f64 {
    let na = normalize(a);
    let nb = normalize(b);
    let max = na.chars().count().max(nb.chars().count());
    if max == 0 {
        return 1.0;
    }
    let dist = levenshtein(&na, &nb);
    1.0 - (dist as f64 / max as f64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fingerprint_ignores_whitespace() {
        assert_eq!(fingerprint("let x = 1;"), fingerprint("let   x =  1;"));
        assert_eq!(fingerprint("a\nb\n"), fingerprint("a b"));
    }

    #[test]
    fn fingerprint_distinguishes_logic() {
        assert_ne!(fingerprint("let x = 1;"), fingerprint("let x = 2;"));
    }

    #[test]
    fn levenshtein_basic() {
        assert_eq!(levenshtein("kitten", "sitting"), 3);
        assert_eq!(levenshtein("", "abc"), 3);
        assert_eq!(levenshtein("abc", "abc"), 0);
    }

    #[test]
    fn similarity_bounds() {
        assert!((similarity("abc", "abc") - 1.0).abs() < f64::EPSILON);
        assert!(similarity("abcdef", "abcdex") > 0.8);
        assert!(similarity("totally", "different words here") < 0.5);
    }
}
