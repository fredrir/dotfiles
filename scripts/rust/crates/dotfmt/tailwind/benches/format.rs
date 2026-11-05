use std::hint::black_box;
use std::sync::Arc;

use criterion::{Criterion, criterion_group, criterion_main};
use dotfmt_tailwind::{Config, OrderTable, Sorter};

const CLASSES: &str = "dark:hover:bg-slate-800 p-4 md:px-6 flex items-center gap-2 bg-[url('/a b.svg')] text-white !mt-2 -m-1 rounded-lg shadow-sm w-full max-w-7xl grid grid-cols-1 md:grid-cols-3";

fn bench_sort(c: &mut Criterion) {
    let mut table = OrderTable::empty();
    for (index, class) in CLASSES.split_whitespace().enumerate() {
        table.insert(class, Some(index as i64));
    }
    let sorter = Sorter::new(Arc::new(table), Config::default());
    c.bench_function("tailwind-sort-class-list", |b| {
        b.iter(|| black_box(sorter.sort_list(black_box(CLASSES))))
    });
}

criterion_group!(benches, bench_sort);
criterion_main!(benches);
