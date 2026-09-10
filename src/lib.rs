#![forbid(unsafe_code)]

//! The fallback guard — a technology of `xmip-core-resilience` (ADR-0048).
//!
//! A failed attempt is answered by the fallback: the guard says so, and the
//! platform hands the caller the word *fallback* instead of the failure. The
//! guard never holds the fallback value — that is the caller's, and a guard
//! never sees a value. Every failure is answered by default; a guard that
//! answers only the permanent ones lets a retryable failure pass, so a retry
//! guard asked after it in the order still gets its attempts.
//!
//! Order matters. The platform stops at the first guard that does not
//! proceed, so a fallback ahead of a retry answers before the retry is asked
//! unless it is told to leave retryable failures alone; a fallback behind a
//! retry answers once the retry has run out.

use resilience::{Attempt, Decision, Guard};

/// The fallback guard: a failure is answered by the fallback.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Fallback {
    only_permanent: bool,
}

impl Fallback {
    /// Answer every failure with the fallback.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            only_permanent: false,
        }
    }

    /// Answer permanent failures only; a retryable failure proceeds, so a
    /// retry guard later in the order may try again.
    #[must_use]
    pub const fn only_permanent(mut self) -> Self {
        self.only_permanent = true;
        self
    }

    /// Whether retryable failures are left to the guards after this one.
    #[must_use]
    pub const fn is_only_permanent(&self) -> bool {
        self.only_permanent
    }
}

impl Guard for Fallback {
    fn technology(&self) -> &'static str {
        "fallback"
    }

    fn before(&self, _: u32) -> Decision {
        Decision::Proceed
    }

    fn after(&self, attempt: &Attempt) -> Decision {
        match &attempt.failure {
            None => Decision::Proceed,
            Some(failure) if self.only_permanent && failure.is_retryable() => Decision::Proceed,
            Some(_) => Decision::Fallback,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use resilience::{Failure, Guarded, execute};
    use std::cell::Cell;
    use std::time::Duration;

    fn attempt(failure: Option<Failure>) -> Attempt {
        Attempt {
            number: 1,
            elapsed: Duration::ZERO,
            failure,
        }
    }

    #[test]
    fn a_failure_is_answered_by_the_fallback_and_a_success_stands() {
        let fallback = Fallback::new();
        assert_eq!(fallback.technology(), "fallback");
        assert_eq!(fallback.before(1), Decision::Proceed);
        assert_eq!(fallback.after(&attempt(None)), Decision::Proceed);
        assert_eq!(
            fallback.after(&attempt(Some(Failure::retryable("again")))),
            Decision::Fallback
        );
        assert_eq!(
            fallback.after(&attempt(Some(Failure::permanent("broken")))),
            Decision::Fallback
        );
        assert!(!fallback.is_only_permanent());
        assert_eq!(Fallback::default(), fallback);
    }

    #[test]
    fn only_permanent_lets_a_retryable_failure_pass() {
        let fallback = Fallback::new().only_permanent();
        assert!(fallback.is_only_permanent());
        assert_eq!(
            fallback.after(&attempt(Some(Failure::retryable("again")))),
            Decision::Proceed
        );
        assert_eq!(
            fallback.after(&attempt(Some(Failure::permanent("broken")))),
            Decision::Fallback
        );
    }

    #[test]
    fn under_execute_a_failed_operation_comes_back_as_the_fallback() {
        let fallback = Fallback::new();
        let guards: [&dyn Guard; 1] = [&fallback];
        let calls = Cell::new(0);
        let fell_back: Result<Guarded<()>, Failure> = execute(&guards, || {
            calls.set(calls.get() + 1);
            Err(Failure::permanent("broken"))
        });
        assert_eq!(fell_back, Ok(Guarded::Fallback));
        assert_eq!(calls.get(), 1, "one attempt, then the fallback answered");

        let done = execute(&guards, || {
            calls.set(calls.get() + 1);
            Ok("done")
        });
        assert_eq!(done, Ok(Guarded::Done("done")));
        assert_eq!(calls.get(), 2);
    }

    /// Tries again on a retryable failure, as the retry technology does.
    struct Again(u32);

    impl Guard for Again {
        fn technology(&self) -> &'static str {
            "retry"
        }

        fn before(&self, _: u32) -> Decision {
            Decision::Proceed
        }

        fn after(&self, attempt: &Attempt) -> Decision {
            match &attempt.failure {
                Some(failure) if failure.is_retryable() && attempt.number < self.0 => {
                    Decision::Wait(Duration::ZERO)
                }
                _ => Decision::Proceed,
            }
        }
    }

    #[test]
    fn a_fallback_behind_retry_answers_once_the_retries_have_run_out() {
        let fallback = Fallback::new();
        let guards: [&dyn Guard; 2] = [&Again(3), &fallback];
        let calls = Cell::new(0);
        let outcome: Result<Guarded<()>, Failure> = execute(&guards, || {
            calls.set(calls.get() + 1);
            Err(Failure::retryable("again"))
        });
        assert_eq!(outcome, Ok(Guarded::Fallback));
        assert_eq!(calls.get(), 3, "retry had its attempts first");
    }

    #[test]
    fn a_fallback_ahead_of_retry_answers_at_once_unless_it_is_only_permanent() {
        let eager = Fallback::new();
        let guards: [&dyn Guard; 2] = [&eager, &Again(3)];
        let calls = Cell::new(0);
        let outcome: Result<Guarded<()>, Failure> = execute(&guards, || {
            calls.set(calls.get() + 1);
            Err(Failure::retryable("again"))
        });
        assert_eq!(outcome, Ok(Guarded::Fallback));
        assert_eq!(
            calls.get(),
            1,
            "the fallback decided before retry was asked"
        );

        let patient = Fallback::new().only_permanent();
        let guards: [&dyn Guard; 2] = [&patient, &Again(3)];
        calls.set(0);
        let outcome: Result<Guarded<()>, Failure> = execute(&guards, || {
            calls.set(calls.get() + 1);
            Err(Failure::retryable("again"))
        });
        assert_eq!(outcome, Err(Failure::retryable("again")));
        assert_eq!(
            calls.get(),
            3,
            "retry got its turn; the failure stayed retryable"
        );

        calls.set(0);
        let outcome: Result<Guarded<()>, Failure> = execute(&guards, || {
            calls.set(calls.get() + 1);
            Err(Failure::permanent("broken"))
        });
        assert_eq!(outcome, Ok(Guarded::Fallback));
        assert_eq!(calls.get(), 1);
    }
}
