use std::hint::black_box;
use std::path::Path;

use criterion::{Criterion, criterion_group, criterion_main};
use dotfmt_css::{Config, Dialect, format};

const SOURCE: &str = r#"
@import "tailwindcss";
@theme {
  --color-brand-50: oklch(0.97 0.02 150);
  --color-brand-500: oklch(0.72 0.19 149.58);
  --font-display: "Satoshi", sans-serif;
}
@custom-variant dark (&:where(.dark, .dark *));
@utility tab-4 { tab-size: 4; }
@layer components {
  .card { display: grid; gap: 1rem; padding: 1.5rem; &:hover { transform: translateY(-2px); } }
  .btn { @apply p-4 flex items-center gap-2 rounded-md font-medium; background: color-mix(in oklab, var(--color-brand-500), white 20%); }
}
@container (min-width: 40rem) { .card { grid-template-columns: 1fr 1fr; } }
@property --angle { syntax: "<angle>"; initial-value: 0deg; inherits: false; }
@starting-style { .card { opacity: 0; } }
"#;

fn bench_format(c: &mut Criterion) {
    let config = Config::default();
    c.bench_function("css-tailwind-format", |b| {
        b.iter(|| {
            black_box(
                format(
                    Path::new("bench.css"),
                    black_box(SOURCE),
                    &config,
                    Dialect::Css,
                )
                .expect("format"),
            )
        })
    });
}

criterion_group!(benches, bench_format);
criterion_main!(benches);
