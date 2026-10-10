# Engineering guidelines

These instructions apply to the Rust `screepsmanager` CLI. They follow docstore's
strict Rust conventions and this crate's existing lint configuration. They do not
apply to the sibling TypeScript bot or OpenScreeps server.

## Design and boundaries

- Correctness first, then maintainability, then measured performance. Prefer
  explicit types and small implementations over speculative abstractions.
- Read the affected implementation and callers before editing. Reuse existing
  patterns and keep unrelated changes out of the patch.
- Keep CLI orchestration in `src/main.rs`, configuration and secret resolution in
  the `config` module (`config.rs`, `config/`) and `envfile.rs`, and HTTP transport
  in the `api` module (`api.rs`, `api/`). Keep upload, polling, selector execution,
  heartbeat validation, and room eligibility in their respective modules rather
  than growing `main.rs` or a generic utilities module.
- Separate pure decisions from filesystem, network, process, and clock access.
  Keep state transitions explicit and preserve the distinction between a refused
  mutation and one whose outcome is unknown.
- Migrate all callers when changing an interface. Remove obsolete helpers and
  branches rather than leaving unused compatibility aliases.

## Size and splitting

- Keep functions below 200 lines. `clippy.toml` sets
  `too-many-lines-threshold = 199`; pedantic Clippy is denied, not advisory, so
  Clippy fails on a function with 200 or more lines of code (it does not count
  comments or blank lines).
- Split earlier when parsing, validation, planning, execution, and presentation
  become separate responsibilities. Helpers need meaningful names and contracts.
- Split hand-written code files when they reach 1,000 lines; keep each resulting
  file below 1,000 lines. Count the whole file, including comments and inline tests.
  `build.rs` enforces this with the standard library only: every `cargo build`,
  `check`, `clippy`, and `test` (and so CI) fails when a Rust file under `src/`,
  `tests/`, `benches/`, or `examples/`, or `build.rs` itself, has 1,000 physical
  lines or more.
- When modifying an existing oversized code file, split it below this limit as
  part of the change. Keep unrelated files out of the refactor.
- Use cohesive submodules, not numbered file fragments, trivial forwarding
  wrappers, compressed statements, or broad lint exemptions to satisfy a limit.
- Split large test modules by behavior. Keep scenario setup and assertions
  understandable together and reuse substantial fixtures without hiding intent.
- Vendored contracts and generated data are exempt from hand-written size targets.
  Update their authoritative source or pinned revision instead of hand-editing them.
- Let rustfmt control Rust layout. Do not introduce manual formatting conventions
  that fight the pinned formatter.

## Strict and pedantic Rust

- Preserve the denied Rust and Clippy lint groups in `Cargo.toml`, including
  warnings, unused code, missing documentation, unsafe code, and pedantic Clippy.
  Do not lower lint levels to make a change pass.
- Do not introduce `#[allow(...)]` attributes. If a lint truly does not apply, use
  the narrowest justified `#[expect(..., reason = "...")]`; explain the invariant
  and let an unfulfilled expectation expose when the exception becomes obsolete.
  Do not use this to bypass unsafe-code restrictions or function-size limits.
- Keep production code safe Rust. Do not add `unsafe`, unchecked conversions, or
  unchecked indexing to avoid expressing a precondition.
- Borrow data when ownership is unnecessary. Do not clone module maps, source
  strings, buffers, or request payloads merely to satisfy an API signature.
- Avoid avoidable allocations, repeated parsing, serialization, and computation.
  Use iterators, slices, and reusable storage when they clarify ownership; do not
  replace readable code with speculative micro-optimizations.
- Prefer domain types and enums over unvalidated strings and combinations of
  booleans. Validate at the boundary, then carry the established invariant inward.
- Use checked numeric conversions and explicit units for durations, timestamps,
  room coordinates, and response sizes. Handle missing fields deliberately.
- Document exported contracts, invariants, side effects, and error conditions.
  Comments explain decisions and constraints rather than restating statements.
- Keep dependencies justified and lockfile-backed. Retain the blocking transport
  unless a concrete requirement justifies changing the runtime model.

## Errors and resource ownership

- Return typed errors for invalid input, configuration, I/O, authentication, and
  server failures. Preserve useful context without exposing credentials or bodies
  that may contain private account data.
- Do not use `unwrap`, `expect`, panics, or fabricated defaults for recoverable
  production failures. A release panic aborts the process; it is not recovery.
  Assertions and `expect` are appropriate for explicit test preconditions.
- Never turn an API failure into an empty account, an eligible room, or permission
  to reset. Fail closed on missing or malformed safety evidence.
- Bound subprocess execution and output. Pass selector arguments directly without
  a shell. Preserve the existing selector protocol and revalidate its answer.
- Own and close sockets, files, child processes, and worker threads. Tests must
  also shut down their listeners and join workers instead of leaking them.

## HTTP contracts and credentials

- The maintained client covers the operations the manager needs. The official
  contract is pinned in `contract/`; `contract/NOTICE` records its provenance and
  update procedure. Preserve its ISC license when updating the snapshot.
- Keep intentional private-server compatibility explicit. Decode required domain
  fields without demanding unrelated official-only fields. Contract boundary tests
  are not a complete JSON Schema validator.
- Do not label endpoint metadata or generated model definitions a generated client.
  Any future generator must preserve borrowed upload bodies, token rotation,
  response error handling, URL subpaths, and the read-only mutation gate.
- Treat HTTP 200 error envelopes as failures. Preserve `X-Token` rotation and
  sensitive-header handling. Do not follow redirects or automatically retry
  mutations whose outcome is unknown.
- Never log, commit, or copy credential values into fixtures or documentation.
  Preserve explicit dotenv loading, environment precedence, and redacted diagnostics.

## Verification

Use the pinned toolchain through mise. Keep `rust-toolchain.toml`, `mise.toml`, and
`Cargo.toml`'s minimum Rust version consistent when intentionally upgrading Rust.

```sh
mise exec -- cargo fmt --all -- --check
mise exec -- cargo clippy --locked --all-targets -- -D warnings
mise exec -- cargo test --locked
mise exec -- cargo build --release --locked
```

`mise run ci` runs the repository's formatting, Clippy, and test tasks. The release
build is separate. The Clippy, test, and build commands also run the `build.rs`
file-size gate, so no separate size command exists. Run focused existing tests while
developing; finish with the complete affected checks before claiming completion.

- Reproduce a bug first and retain a deterministic regression for observable
  behavior, boundaries, invariants, or error handling.
- Test failure paths and ambiguous mutation outcomes, not just successful replies.
  Do not add source-text checks, mock echoes, or implementation-detail assertions.
- Exercise CLI behavior against a disposable real OpenScreeps server after changes
  to transport, uploads, configuration, or polling. Observe server state as well
  as command output; successful compilation is not behavioral proof.
- Update callers, tests, and documentation together. Report only checks actually
  run, including failures and any unverified platform assumptions.
- For documentation-only edits, check the changed document's formatting and
  commands against the repository. Rust tests are not required for prose changes.

## Live operations and releases

- Code changes do not authorize official-server uploads, branch activation, CPU
  allocation, respawning, or spawn placement. Use disposable local worlds for
  mutation tests; obtain explicit authorization for live operations.
- Polling remains read-only unless `--execute` is supplied. Preserve heartbeat
  freshness, zero-foothold evidence, confirmation windows, cooldowns, and final
  revalidation before any reset or placement.
- Do not stop an existing poller just to rebuild its executable. On Windows, build
  to a separate directory with `--target-dir target/verification` if it is locked.
- Keep the executable and configuration names stable unless a rename is requested.
  Public repository links use `openscreeps/screepsmanager`.
- For an authorized release, update `Cargo.toml` and `Cargo.lock`, pass the checks,
  and use a matching version tag. Let the existing workflows build and publish
  archives, checksums, and provenance; do not replace already-published artifacts.
