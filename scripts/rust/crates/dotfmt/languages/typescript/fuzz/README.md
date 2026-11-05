# Fuzzing

```sh
cargo install cargo-fuzz
cd scripts/rust/crates/dotfmt/languages/typescript
cargo fuzz run format
```

The target treats arbitrary UTF-8 as TSX, formats it, and runs analysis with
fixes enabled. A syntax error is a normal result; a panic is a bug.
