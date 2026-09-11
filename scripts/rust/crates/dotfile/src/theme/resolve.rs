use super::{
    Result,
    color::Color,
    model::{Theme, table, text},
};
use std::rc::Rc;
#[derive(Clone)]
pub struct Foreground {
    pub key: String,
    pub color: Color,
    pub floor: f64,
}
#[derive(Clone)]
pub struct KdeGroup {
    pub name: String,
    pub backgrounds: [Color; 2],
    pub decoration: Color,
    pub foregrounds: Vec<Foreground>,
}
pub fn kde(t: &Theme) -> Result<Rc<Vec<KdeGroup>>> {
    if let Some(groups) = t.kde.borrow().as_ref() {
        return Ok(groups.clone());
    }
    let spec = t.map("kde")?;
    let mut groups = Vec::new();
    for (name, values) in table(&spec["groups"])? {
        let backgrounds = [
            t.app("kde", text(&values[0]))?,
            t.app("kde", text(&values[1]))?,
        ];
        let decoration =
            t.readable_many(text(&t.data.roles["kde"]["decoration"]), &backgrounds, 3.)?;
        let mut foregrounds = Vec::new();
        for (key, role) in table(&spec["foregrounds"])? {
            let floor = if text(role) == "inactive" { 3. } else { 4.5 };
            let color = if name == "Colors:Selection" {
                t.app("kde", text(&spec["selection_foregrounds"][key]))?
            } else {
                t.readable_many(text(&t.data.roles["kde"][text(role)]), &backgrounds, floor)?
            };
            foregrounds.push(Foreground {
                key: key.clone(),
                color,
                floor,
            });
        }
        groups.push(KdeGroup {
            name: name.clone(),
            backgrounds,
            decoration,
            foregrounds,
        });
    }
    let groups = Rc::new(groups);
    *t.kde.borrow_mut() = Some(groups.clone());
    Ok(groups)
}
