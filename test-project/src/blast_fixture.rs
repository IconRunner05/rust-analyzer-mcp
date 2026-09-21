//! A symbol reached from production code and from tests, both living in this one file.
//!
//! This is the shape that decides whether a test/production split is real. Rust puts a crate's
//! unit tests in a `#[cfg(test)] mod tests` beside the code they test, so every caller below is
//! in the same file as every other -- and a split made by filename has no way to tell them apart.
//! It reports four production callers and no tests, which reads as a symbol the suite does not
//! cover.
//!
//! The fixture discriminates a second rule that is wrong in the other direction. One of the two
//! production callers carries a doctest, and a split that trusts a doctest runnable's range files
//! that caller under `tests` -- one production caller and three tests. The real answer is two and
//! two, so no count here is reachable by either wrong rule.

/// The symbol a blast radius is taken of.
pub fn shared_helper(value: i32) -> i32 {
    value + 1
}

/// Production code calling it, documented with no code fence. The control: it says the variable
/// below is the doctest and not merely the presence of rustdoc.
pub fn production_caller(value: i32) -> i32 {
    shared_helper(value)
}

/// Production code calling it from a function that carries a doctest.
///
/// rust-analyzer offers to run that doctest and reports the offer's range as *this whole
/// function*, signature and body included, rather than the fenced lines above. Counting that
/// range as test code files the call below under `tests`, and on a crate whose standard is to
/// document its public surface that is most of the callers there are.
///
/// ```
/// assert_eq!(test_project::blast_fixture::documented_caller(1), 2);
/// ```
pub fn documented_caller(value: i32) -> i32 {
    shared_helper(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Test code that is not itself a `#[test]`, so it has no runnable of its own and is covered
    /// only by the one belonging to the module around it.
    fn a_test_helper(value: i32) -> i32 {
        shared_helper(value)
    }

    #[test]
    fn the_helper_adds_one() {
        assert_eq!(shared_helper(1), 2);
        assert_eq!(a_test_helper(1), 2);
    }
}
