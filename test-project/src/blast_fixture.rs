//! A symbol reached from production code and from tests, both living in this one file.
//!
//! This is the shape that decides whether a test/production split is real. Rust puts a crate's
//! unit tests in a `#[cfg(test)] mod tests` beside the code they test, so every caller below is
//! in the same file as every other -- and a split made by filename has no way to tell them apart.
//! It reports three production callers and no tests, which reads as a symbol the suite does not
//! cover.

/// The symbol a blast radius is taken of.
pub fn shared_helper(value: i32) -> i32 {
    value + 1
}

/// Production code calling it. The only caller that should be counted as production.
pub fn production_caller(value: i32) -> i32 {
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
