use std::time::Duration;

use crate::cache::{self, FirstUse, Source};
use crate::context::Context;
use crate::node::npmrc::Npmrc;
use crate::node::registry::{self, Hit};

// npm packages tagged the way the pi.dev gallery finds them, most downloaded first.
pub struct Gallery;

impl Source for Gallery {
    type Value = Vec<Hit>;

    fn key(&self) -> String {
        "pi-gallery".into()
    }

    fn stamp(&self, _ctx: &Context) -> String {
        String::new()
    }

    fn ttl(&self) -> Option<Duration> {
        Some(Duration::from_secs(24 * 3600))
    }

    fn job(&self) -> Vec<String> {
        vec!["pi-gallery".into()]
    }

    fn first_use(&self) -> FirstUse {
        FirstUse::Background
    }

    fn build(&self, ctx: &Context) -> Option<Vec<Hit>> {
        let registry = Npmrc::load(ctx).registry_for("");
        let mut hits = registry::query(
            ctx,
            &registry,
            "keywords:pi-package",
            "250",
            Duration::from_secs(15),
        )?;
        hits.sort_by_key(|hit| std::cmp::Reverse(hit.downloads));
        Some(hits)
    }
}

pub fn packages(ctx: &Context) -> Vec<Hit> {
    cache::load(ctx, &Gallery).unwrap_or_default()
}
