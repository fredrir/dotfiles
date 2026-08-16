//! Bounded fuzzing for the lexer and parser (TC03-FUZZ, M1 exit gate).
//!
//! A deterministic PRNG drives four generators: arbitrary bytes, grammar-
//! biased token soup, mutations of valid documents, and adversarial nesting.
//! Every generated input must satisfy the same invariants: no panic, exact
//! byte replay, the retained-diagnostic limit, and identical results across
//! repeated runs.

use dotfile_source::{LineIndex, PositionEncoding, RepoPath, SourceText};
use dotfile_syntax::parse;

/// xorshift64* — small deterministic PRNG, no external dependencies.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545F4914F6CDD1D)
    }

    fn below(&mut self, bound: usize) -> usize {
        (self.next() % bound.max(1) as u64) as usize
    }
}

const SEEDS: &[&[u8]] = &[
    b"foo\n",
    b"@let vault = \"~/main\"\n@destination = \"${vault}/.obsidian\"\n",
    b"wezterm {\n    @version = \"1\"\n    hammerspoon\n}\n",
    b"a = [\n    \"x\",\n    \"y\",\n]\n",
    b"./.zshrc { @destination = \"~/.zshrc\" }\n",
    b"?@font {\n    @key = hack_nerd_font\n}\n",
    b"@extend entity/wezterm {\n    @pkg = \"wezterm\"\n}\n",
    "\u{feff}# comment\nfoo = \"héllo \u{1f600}\"\n".as_bytes(),
];

fn arbitrary_bytes(rng: &mut Rng, length: usize) -> Vec<u8> {
    (0..length).map(|_| rng.next() as u8).collect()
}

fn token_soup(rng: &mut Rng, length: usize) -> Vec<u8> {
    const PIECES: &[&[u8]] = &[
        b"foo",
        b"7z",
        b".zshrc",
        b"=",
        b"{",
        b"}",
        b"[",
        b"]",
        b",",
        b"@",
        b"$",
        b"?",
        b"/",
        b"@let",
        b"@extend",
        b"\"string\"",
        b"\"${v}\"",
        b"./path",
        b"./\"q p\"",
        b"\n",
        b"\r\n",
        b" ",
        b"\t",
        b"# c\n",
        b"if",
        b"null",
        b"\\",
        b"\"",
        b"\xef\xbb\xbf",
        "héllo".as_bytes(),
    ];
    let mut output = Vec::new();
    while output.len() < length {
        output.extend_from_slice(PIECES[rng.below(PIECES.len())]);
    }
    output.truncate(length);
    output
}

fn mutated(rng: &mut Rng, seed: &[u8]) -> Vec<u8> {
    let mut bytes = seed.to_vec();
    for _ in 0..1 + rng.below(4) {
        match rng.below(4) {
            0 if !bytes.is_empty() => {
                let at = rng.below(bytes.len());
                bytes[at] = rng.next() as u8;
            }
            1 if !bytes.is_empty() => {
                let at = rng.below(bytes.len());
                bytes.remove(at);
            }
            2 => {
                let at = rng.below(bytes.len() + 1);
                bytes.insert(at, rng.next() as u8);
            }
            _ => {
                let at = rng.below(bytes.len());
                bytes.truncate(at);
            }
        }
    }
    bytes
}

fn adversarial_nesting(rng: &mut Rng) -> Vec<u8> {
    let mut output = Vec::new();
    let (open, close) = if rng.below(2) == 0 {
        (b'{', b'}')
    } else {
        (b'[', b']')
    };
    let depth = 200 + rng.below(200);
    for _ in 0..depth {
        output.push(open);
    }
    for _ in 0..depth + rng.below(10) {
        output.push(close);
    }
    output
}

fn check_invariants(input: &[u8]) {
    let path = RepoPath::new("fixture.dotfile").unwrap();
    let source = SourceText::from_bytes(input.to_vec());
    let first = parse(&path, &source);
    let second = parse(&path, &source);

    // Exact byte replay, including invalid and recovered regions.
    assert_eq!(first.cst().replay(input), input, "replay mismatch");
    // Determinism across repeated runs.
    assert_eq!(
        first.cst().dump(input),
        second.cst().dump(input),
        "nondeterministic CST"
    );
    assert_eq!(
        serde_json::to_value(first.diagnostics()).unwrap(),
        serde_json::to_value(second.diagnostics()).unwrap(),
        "nondeterministic diagnostics"
    );
    // The retained-diagnostic limit holds.
    assert!(first.diagnostics().len() <= 512);
    // Lexer gap/token invariants hold.
    first.lexed_invariants(input);
}

trait LexedInvariants {
    fn lexed_invariants(&self, input: &[u8]);
}

impl LexedInvariants for dotfile_syntax::Parse {
    fn lexed_invariants(&self, input: &[u8]) {
        let tokens = self.cst().tokens();
        let gaps = self.cst().gaps();
        assert_eq!(gaps.len(), tokens.len() + 1);
        let mut cursor = 0;
        for (index, token) in tokens.iter().enumerate() {
            assert_eq!(gaps[index].range.start(), cursor);
            assert_eq!(gaps[index].range.end(), token.range.start());
            cursor = token.range.end();
        }
        assert_eq!(gaps[tokens.len()].range.start(), cursor);
        assert_eq!(gaps[tokens.len()].range.end(), input.len() as u64);
    }
}

#[test]
fn fuzz_arbitrary_bytes() {
    let mut rng = Rng(0xD0A7F11E);
    for _ in 0..200 {
        let length = 1 + rng.below(200);
        let input = arbitrary_bytes(&mut rng, length);
        check_invariants(&input);
    }
}

#[test]
fn fuzz_token_soup() {
    let mut rng = Rng(0x5EEDCAFE);
    for _ in 0..200 {
        let length = 1 + rng.below(300);
        let input = token_soup(&mut rng, length);
        check_invariants(&input);
    }
}

#[test]
fn fuzz_mutations_of_valid_documents() {
    let mut rng = Rng(0xB16B00B5);
    for index in 0..200 {
        let seed = SEEDS[index % SEEDS.len()];
        let input = mutated(&mut rng, seed);
        check_invariants(&input);
    }
}

#[test]
fn fuzz_adversarial_nesting_is_bounded() {
    let mut rng = Rng(0xDEADBEEF);
    for _ in 0..200 {
        let input = adversarial_nesting(&mut rng);
        check_invariants(&input);
    }
}

#[test]
fn fuzz_escape_and_interpolation_storms() {
    let mut rng = Rng(0xE5CA9E);
    const PIECES: &[&[u8]] = &[
        b"\\u{", b"}", b"\\", b"\"", b"${", b"$", b"\\$", b"\\n", b"0", b"abcdef", b"\"",
    ];
    for _ in 0..200 {
        let mut input = Vec::new();
        for _ in 0..1 + rng.below(40) {
            input.extend_from_slice(PIECES[rng.below(PIECES.len())]);
        }
        check_invariants(&input);
    }
}

#[test]
fn span_round_trips_on_generated_documents() {
    let mut rng = Rng(0xC00D1E5);
    for _ in 0..200 {
        let length = 1 + rng.below(200);
        let input = token_soup(&mut rng, length);
        let index = LineIndex::new(&input);
        for offset in 0..=input.len() as u64 {
            if !index.is_anchor_boundary(&input, offset) {
                continue;
            }
            let dotfile_source::LineCol { line, column } = index.line_col(&input, offset);
            assert_eq!(index.offset(&input, line, column), Some(offset));
            for encoding in [PositionEncoding::Utf8, PositionEncoding::Utf16] {
                let (lsp_line, character) = index.lsp_position(&input, offset, encoding);
                assert_eq!(
                    index.offset_at_lsp(&input, lsp_line, character, encoding),
                    Some(offset)
                );
            }
        }
    }
}
