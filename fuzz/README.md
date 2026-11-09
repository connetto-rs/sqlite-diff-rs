# Fuzzing for sqlite-diff-rs

This directory contains harnesses for fuzz testing the `sqlite-diff-rs` crate.

## What is Fuzzing?

[Fuzzing](https://rust-fuzz.github.io/book/) is an automated testing technique that feeds random, invalid, or unexpected inputs into your program to find bugs, crashes, or security vulnerabilities. We use [libFuzzer](https://llvm.org/docs/LibFuzzer.html) through [cargo-fuzz](https://github.com/rust-fuzz/cargo-fuzz).

## Getting Started

1. **Install cargo-fuzz** (needs a nightly toolchain)

   ```bash
   cargo install cargo-fuzz
   ```

2. **Run a Fuzzer**

   ```bash
   cargo +nightly fuzz run roundtrip -- -timeout=5
   cargo +nightly fuzz run sql_roundtrip -- -timeout=5
   cargo +nightly fuzz run apply_roundtrip -- -timeout=5
   ```

3. **Debugging Crashes**

   If a crash is found, the input is saved in `fuzz/artifacts/<target>/`. You can replay it with:

   ```bash
   cargo +nightly fuzz run roundtrip fuzz/artifacts/roundtrip/crash-<hash>
   ```

   `cargo test --all-features --test fuzz_regression` copies every artifact into `tests/crash_inputs/<target>/` and replays the whole directory.

## Seed Corpus

Every target has a seed corpus in `fuzz/seeds/<target>/`, one input per file. `roundtrip` and `reverse_idempotent` start from real rusqlite session changesets and patchsets, and the `wire_*` targets start from one valid value per wire type. `apply_roundtrip`, `sql_roundtrip` and `differential` decode their input through `arbitrary`, so their seeds are fuzzer-grown inputs reduced by libFuzzer's `-set_cover_merge=1` to the smallest set that keeps the same coverage. Pass the seeds as a second corpus directory to start a local run from them:

```bash
mkdir -p fuzz/corpus/roundtrip
cargo +nightly fuzz run roundtrip fuzz/corpus/roundtrip fuzz/seeds/roundtrip
```

## Fuzz Targets

`roundtrip` checks binary round-trip stability: parse arbitrary bytes into a `ParsedDiffSet`, serialize back to bytes, re-parse, and re-serialize. The two serialized byte sequences must be identical, which proves a single normalization pass produces stable output. This also exercises parser robustness on arbitrary input.

`sql_roundtrip` checks SQL `Display` round-trip: parse SQL into a `ChangeSet` or `PatchSet`, convert back to SQL via `Display`, re-parse, and compare the in-memory structures.

`apply_roundtrip` parses arbitrary bytes as a binary changeset or patchset, serializes back and asserts byte equality, then applies the re-serialized changeset to an in-memory rusqlite database. It returns early on parse failure to keep iteration cost bounded, and input size is capped at 4 KiB.
