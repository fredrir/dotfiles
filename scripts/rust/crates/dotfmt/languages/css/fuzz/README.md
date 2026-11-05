# Fuzzing

```sh
cargo install cargo-fuzz
cd scripts/rust/crates/dotfmt/languages/css
cargo fuzz run format
```

The target treats arbitrary UTF-8 as each CSS dialect; a syntax error is a
normal result, a panic is a bug.
