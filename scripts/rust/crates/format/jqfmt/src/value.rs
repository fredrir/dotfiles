use indexmap::IndexMap;

/// A JSON value as jq holds one: a number keeps its coefficient instead of
/// becoming a float, so `1.10` and a forty digit integer survive a round trip.
#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Null,
    Bool(bool),
    Number(String),
    String(String),
    Array(Vec<Value>),
    /// Insertion ordered with room for a repeated key, because that is what jq
    /// answers with: the last value lands on the first position.
    Object(IndexMap<String, Value>),
}
