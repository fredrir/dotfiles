#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Null,
    Bool(bool),
    Number(String),
    String(String),
    Array(Vec<Value>),
    /// Members in the order they were written, repeats included: JSON allows a
    /// key to appear more than once and a formatter has no business dropping
    /// one of them.
    Object(Vec<(String, Value)>),
}
