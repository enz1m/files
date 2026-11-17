# Tests

Run the normal Cargo checks from the repository root:

```sh
cargo fmt --all -- --check
ICED_TEST_BACKEND=software cargo test --locked --workspace -- --test-threads=1
cargo clippy --locked --workspace --all-targets -- -D warnings
cargo check --locked --workspace --all-targets --all-features
cargo check --locked --manifest-path extensions/examples/hello/Cargo.toml
```

The suite covers filesystem publication and rollback, conflicts, cancellation,
trash and undo identity, bounded persistence, malformed archives, extension
permissions, credential handling, stale replies and session recovery.
