use std::sync::OnceLock;
use std::time::Duration;

use serde::de::DeserializeOwned;

use crate::context::Context;

const CONNECT_TIMEOUT: Duration = Duration::from_millis(600);
const BODY_LIMIT: u64 = 64 * 1024 * 1024;

pub struct Request<'a> {
    url: &'a str,
    query: Vec<(&'a str, &'a str)>,
    accept: &'a str,
    timeout: Duration,
}

impl<'a> Request<'a> {
    pub fn get(url: &'a str) -> Request<'a> {
        Request {
            url,
            query: Vec::new(),
            accept: "application/json",
            timeout: Duration::from_secs(10),
        }
    }

    pub fn query(mut self, name: &'a str, value: &'a str) -> Request<'a> {
        self.query.push((name, value));
        self
    }

    pub fn accept(mut self, accept: &'a str) -> Request<'a> {
        self.accept = accept;
        self
    }

    pub fn timeout(mut self, timeout: Duration) -> Request<'a> {
        self.timeout = timeout;
        self
    }

    pub fn json<T: DeserializeOwned>(self, ctx: &Context) -> Option<T> {
        if ctx.offline {
            return None;
        }
        let mut request = agent()
            .get(self.url)
            .header("Accept", self.accept)
            .config()
            .timeout_global(Some(self.timeout))
            .build();
        for (name, value) in self.query {
            request = request.query(name, value);
        }
        let mut response = request.call().ok()?;
        response
            .body_mut()
            .with_config()
            .limit(BODY_LIMIT)
            .read_json()
            .ok()
    }
}

fn agent() -> &'static ureq::Agent {
    static AGENT: OnceLock<ureq::Agent> = OnceLock::new();
    AGENT.get_or_init(|| {
        ureq::Agent::config_builder()
            .timeout_connect(Some(CONNECT_TIMEOUT))
            .user_agent(concat!("zcomp/", env!("CARGO_PKG_VERSION")))
            .build()
            .into()
    })
}
