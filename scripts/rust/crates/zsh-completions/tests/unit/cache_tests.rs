use std::cell::Cell;

use super::*;
use testkit::tree;

struct Counter {
    stamp: &'static str,
    ttl: Option<Duration>,
    first: FirstUse,
    value: Option<u32>,
    builds: Cell<u32>,
}

impl Counter {
    fn new(value: Option<u32>) -> Counter {
        Counter {
            stamp: "one",
            ttl: None,
            first: FirstUse::Build,
            value,
            builds: Cell::new(0),
        }
    }
}

impl Source for Counter {
    type Value = u32;

    fn key(&self) -> String {
        "counter".into()
    }

    fn stamp(&self, _ctx: &Context) -> String {
        self.stamp.into()
    }

    fn ttl(&self) -> Option<Duration> {
        self.ttl
    }

    // No job, so a stale value is never handed to a background process.
    fn job(&self) -> Vec<String> {
        Vec::new()
    }

    fn build(&self, _ctx: &Context) -> Option<u32> {
        self.builds.set(self.builds.get() + 1);
        self.value
    }

    fn first_use(&self) -> FirstUse {
        self.first
    }
}

fn background(ctx: &Context) -> Context {
    let mut ctx = ctx.clone();
    ctx.foreground = false;
    ctx
}

#[test]
fn a_value_is_built_once_and_then_read_back() {
    let root = tree(&[]);
    let ctx = Context::testing(root.path(), root.path(), &[]);
    let source = Counter::new(Some(7));
    assert_eq!(load(&ctx, &source), Some(7));
    assert_eq!(load(&ctx, &source), Some(7));
    assert_eq!(source.builds.get(), 1);
}

#[test]
fn a_changed_stamp_rebuilds_in_the_foreground() {
    let root = tree(&[]);
    let ctx = Context::testing(root.path(), root.path(), &[]);
    load(&ctx, &Counter::new(Some(1)));
    let changed = Counter {
        stamp: "two",
        ..Counter::new(Some(2))
    };
    assert_eq!(load(&ctx, &changed), Some(2));
    assert_eq!(changed.builds.get(), 1);
}

#[test]
fn a_stale_value_is_served_while_the_rebuild_is_left_to_the_background() {
    let root = tree(&[]);
    let ctx = Context::testing(root.path(), root.path(), &[]);
    load(&ctx, &Counter::new(Some(1)));
    let changed = Counter {
        stamp: "two",
        ..Counter::new(Some(2))
    };
    assert_eq!(load(&background(&ctx), &changed), Some(1));
    assert_eq!(changed.builds.get(), 0);
}

#[test]
fn an_expired_value_counts_as_stale() {
    let root = tree(&[]);
    let mut ctx = Context::testing(root.path(), root.path(), &[]);
    let source = Counter {
        ttl: Some(Duration::from_secs(60)),
        ..Counter::new(Some(1))
    };
    load(&ctx, &source);
    ctx.now += Duration::from_secs(120);
    load(&ctx, &source);
    assert_eq!(source.builds.get(), 2);
}

#[test]
fn a_background_source_builds_nothing_on_first_use() {
    let root = tree(&[]);
    let ctx = background(&Context::testing(root.path(), root.path(), &[]));
    let source = Counter {
        first: FirstUse::Background,
        ..Counter::new(Some(1))
    };
    assert_eq!(load(&ctx, &source), None);
    assert_eq!(source.builds.get(), 0);
}

#[test]
fn a_failed_build_keeps_the_stored_value() {
    let root = tree(&[]);
    let ctx = Context::testing(root.path(), root.path(), &[]);
    load(&ctx, &Counter::new(Some(1)));
    let failing = Counter {
        stamp: "two",
        ..Counter::new(None)
    };
    assert_eq!(load(&ctx, &failing), Some(1));
}

#[test]
fn a_refresh_stores_its_value_and_releases_the_lock() {
    let root = tree(&[]);
    let ctx = Context::testing(root.path(), root.path(), &[]);
    assert!(take_lock(&ctx, "counter"));
    assert!(refresh(&ctx, &Counter::new(Some(9))));
    assert_eq!(peek::<u32>(&ctx, "counter"), Some(9));
    assert!(take_lock(&ctx, "counter"), "the lock was released");
}

#[test]
fn a_lock_is_taken_once_until_it_expires() {
    let root = tree(&[]);
    let mut ctx = Context::testing(root.path(), root.path(), &[]);
    assert!(take_lock(&ctx, "job"));
    assert!(!take_lock(&ctx, "job"));
    ctx.now += LOCK_EXPIRY * 2;
    assert!(take_lock(&ctx, "job"));
}

#[test]
fn stored_values_report_their_age() {
    let root = tree(&[]);
    let mut ctx = Context::testing(root.path(), root.path(), &[]);
    store(&ctx, "plain", &"value");
    ctx.now += Duration::from_secs(30);
    assert_eq!(
        peek_aged::<String>(&ctx, "plain"),
        Some(("value".into(), 30))
    );
}

#[test]
fn keys_map_to_distinct_file_names() {
    assert_eq!(
        file_name("search-registry.npmjs.org-react"),
        "search-registry.npmjs.org-react"
    );
    assert_ne!(file_name("a/b"), file_name("a_b"));
    assert_ne!(file_name("a/b"), file_name("a_2fb"));
    assert!(!file_name("@types/node").contains('/'));
}
