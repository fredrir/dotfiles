use crate::{Directory, InputKind};

pub trait FileSource {
    type Location: Clone + Eq;
    type Error;

    fn read_directory(
        &self,
        location: &Self::Location,
    ) -> Result<Directory<Self::Location>, Self::Error>;

    fn refresh_directory(
        &self,
        location: &Self::Location,
    ) -> Result<Directory<Self::Location>, Self::Error> {
        self.read_directory(location)
    }

    fn input_kind(&self, _text: &str) -> InputKind {
        InputKind::Search
    }

    fn resolve_input(
        &self,
        current: &Self::Location,
        _text: &str,
    ) -> Result<Self::Location, Self::Error> {
        Ok(current.clone())
    }

    fn prefetch(&self, _location: &Self::Location) {}
}
