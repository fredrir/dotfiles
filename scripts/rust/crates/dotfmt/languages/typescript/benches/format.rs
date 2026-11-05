use std::hint::black_box;
use std::path::Path;

use criterion::{Criterion, criterion_group, criterion_main};
use dotfmt_typescript::{Config, Dialect, analyze, format};

const SOURCE: &str = r#"
import { useEffect, useMemo, useState } from "react";
import { cn } from "@/lib/utils";

type Item = { id: string; label: string; active?: boolean };

export function Panel({ items, title }: { items: Item[]; title: string }) {
  const [open, setOpen] = useState(false);
  const visible = useMemo(() => items.filter((item) => item.active), [items]);
  useEffect(() => {
    if (!open) return;
    const onKey = (event: KeyboardEvent) => {
      if (event.key === "Escape") setOpen(false);
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [open]);
  return (
    <section className={cn("flex flex-col gap-4 p-6", open && "bg-slate-950 text-white")}>
      <header className="flex items-center justify-between">
        <h2 className="text-lg font-semibold">{title}</h2>
        <button type="button" onClick={() => setOpen((value) => !value)}>
          {open ? "Close" : "Open"}
        </button>
      </header>
      <ul className="grid grid-cols-1 gap-2 md:grid-cols-2">
        {visible.map((item) => (
          <li key={item.id} className="rounded-md border border-slate-200 p-3">
            {item.label}
          </li>
        ))}
      </ul>
    </section>
  );
}
"#;

fn bench_format(c: &mut Criterion) {
    let config = Config::default();
    c.bench_function("typescript-tsx-format", |b| {
        b.iter(|| {
            black_box(
                format(
                    Path::new("bench.tsx"),
                    black_box(SOURCE),
                    &config,
                    Dialect::Tsx,
                )
                .expect("format"),
            )
        })
    });
}

fn bench_analyze(c: &mut Criterion) {
    let config = Config::default();
    let options = analyze::Options {
        fix: true,
        ..Default::default()
    };
    c.bench_function("typescript-tsx-analyze", |b| {
        b.iter(|| {
            black_box(
                analyze(
                    Path::new("bench.tsx"),
                    black_box(SOURCE),
                    &config,
                    Dialect::Tsx,
                    &options,
                )
                .expect("analyze"),
            )
        })
    });
}

criterion_group!(benches, bench_format, bench_analyze);
criterion_main!(benches);
