# File locks and parallel child-process tests

The `table` integration binary runs independent tests in parallel. Its fault
checks start child copies of itself. On Unix, starting a child can temporarily
clone every open file description before exec closes the CLOEXEC descriptors;
other parent threads can continue running during that interval. CLOEXEC prevents
inheritance after exec, not the transient reference before it.

Linux flock and open-file-description range locks are released on the last
close, including a child's reference. Therefore the file test's `close releases`
assertion can race a concurrent child launch even though the provider correctly
closes its own descriptor. Changing the provider to unlock before closing would
change the locking semantics; accepting WOULD_BLOCK or retrying the assertion
would weaken the last-close test instead of controlling its precondition.

A shared test-only mutex now excludes child creation from the file test that
requires sole ownership of the open descriptions. The assertions are unchanged.
The explicit worker threads that test waiting locks, and the fault test's own
worker thread, still execute concurrently. The suite is not globally serialized.

`lock_inheritance.rs` deterministically keeps a forked child alive with a pipe,
closes the parent's locked handle, verifies WOULD_BLOCK while the child retains
the description, and verifies success after the child exits and is reaped. It
covers flock and OFD range locks against the actual Std provider. The child uses
only async-signal-safe operations and is also cleaned up on parent test failure.

The file-lock regression workflow runs this test and the entire table suite
20 times at 16 test threads on both Linux x64 and ARM64, preserving each log.
