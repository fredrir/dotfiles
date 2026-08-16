use std::collections::BTreeMap;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Capabilities {
    pub create_no_replace: bool,
    pub guarded_replace: bool,
    pub guarded_prune: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ObjectToken {
    volume: u64,
    file: u64,
    generation: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GuardedError {
    Absent,
    AlreadyExists,
    TokenMismatch,
    Unsupported,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct Entry {
    bytes: Vec<u8>,
    token: ObjectToken,
}

#[derive(Debug)]
pub struct FakeFilesystem {
    capabilities: Capabilities,
    entries: BTreeMap<String, Entry>,
    next_file: u64,
    next_generation: u64,
}

impl FakeFilesystem {
    pub fn new(capabilities: Capabilities) -> Self {
        Self {
            capabilities,
            entries: BTreeMap::new(),
            next_file: 1,
            next_generation: 1,
        }
    }

    pub fn bytes(&self, path: &str) -> Option<&[u8]> {
        self.entries.get(path).map(|entry| entry.bytes.as_slice())
    }

    pub fn token(&self, path: &str) -> Option<ObjectToken> {
        self.entries.get(path).map(|entry| entry.token)
    }

    pub fn create_no_replace(
        &mut self,
        path: impl Into<String>,
        bytes: Vec<u8>,
    ) -> Result<ObjectToken, GuardedError> {
        if !self.capabilities.create_no_replace {
            return Err(GuardedError::Unsupported);
        }
        let path = path.into();
        if self.entries.contains_key(&path) {
            return Err(GuardedError::AlreadyExists);
        }
        Ok(self.insert_new(path, bytes))
    }

    pub fn insert_foreign(&mut self, path: impl Into<String>, bytes: Vec<u8>) -> ObjectToken {
        self.insert_new(path.into(), bytes)
    }

    pub fn modify(&mut self, path: &str, bytes: Vec<u8>) -> Result<ObjectToken, GuardedError> {
        if !self.entries.contains_key(path) {
            return Err(GuardedError::Absent);
        }
        let generation = self.take_generation();
        let entry = self.entries.get_mut(path).unwrap();
        entry.bytes = bytes;
        entry.token.generation = generation;
        Ok(entry.token)
    }

    pub fn recreate(&mut self, path: impl Into<String>, bytes: Vec<u8>) -> ObjectToken {
        let path = path.into();
        self.entries.remove(&path);
        self.insert_new(path, bytes)
    }

    pub fn guarded_replace(
        &mut self,
        path: &str,
        expected: ObjectToken,
        bytes: Vec<u8>,
    ) -> Result<ObjectToken, GuardedError> {
        if !self.capabilities.guarded_replace {
            return Err(GuardedError::Unsupported);
        }
        let current = self.token(path).ok_or(GuardedError::Absent)?;
        if current != expected {
            return Err(GuardedError::TokenMismatch);
        }
        let generation = self.take_generation();
        let file = self.take_file();
        let token = ObjectToken {
            volume: current.volume,
            file,
            generation,
        };
        self.entries.insert(path.to_owned(), Entry { bytes, token });
        Ok(token)
    }

    pub fn guarded_prune(&mut self, path: &str, expected: ObjectToken) -> Result<(), GuardedError> {
        if !self.capabilities.guarded_prune {
            return Err(GuardedError::Unsupported);
        }
        let current = self.token(path).ok_or(GuardedError::Absent)?;
        if current != expected {
            return Err(GuardedError::TokenMismatch);
        }
        self.entries.remove(path);
        Ok(())
    }

    fn insert_new(&mut self, path: String, bytes: Vec<u8>) -> ObjectToken {
        let token = ObjectToken {
            volume: 1,
            file: self.take_file(),
            generation: self.take_generation(),
        };
        self.entries.insert(path, Entry { bytes, token });
        token
    }

    fn take_file(&mut self) -> u64 {
        let value = self.next_file;
        self.next_file += 1;
        value
    }

    fn take_generation(&mut self) -> u64 {
        let value = self.next_generation;
        self.next_generation += 1;
        value
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn supported() -> Capabilities {
        Capabilities {
            create_no_replace: true,
            guarded_replace: true,
            guarded_prune: true,
        }
    }

    #[test]
    fn create_refuses_an_existing_name() {
        let mut filesystem = FakeFilesystem::new(supported());
        filesystem
            .create_no_replace("target", b"first".to_vec())
            .unwrap();

        assert_eq!(
            filesystem.create_no_replace("target", b"second".to_vec()),
            Err(GuardedError::AlreadyExists)
        );
        assert_eq!(filesystem.bytes("target"), Some(b"first".as_slice()));
    }

    #[test]
    fn replace_compares_and_mutates_as_one_model_operation() {
        let mut filesystem = FakeFilesystem::new(supported());
        let token = filesystem.insert_foreign("target", b"first".to_vec());

        let replacement = filesystem
            .guarded_replace("target", token, b"second".to_vec())
            .unwrap();

        assert_ne!(replacement, token);
        assert_eq!(filesystem.bytes("target"), Some(b"second".as_slice()));
    }

    #[test]
    fn replacement_with_a_stale_token_makes_no_mutation() {
        let mut filesystem = FakeFilesystem::new(supported());
        let stale = filesystem.insert_foreign("target", b"first".to_vec());
        filesystem.modify("target", b"foreign".to_vec()).unwrap();

        assert_eq!(
            filesystem.guarded_replace("target", stale, b"desired".to_vec()),
            Err(GuardedError::TokenMismatch)
        );
        assert_eq!(filesystem.bytes("target"), Some(b"foreign".as_slice()));
    }

    #[test]
    fn pruning_with_a_stale_token_makes_no_mutation() {
        let mut filesystem = FakeFilesystem::new(supported());
        let stale = filesystem.insert_foreign("target", b"first".to_vec());
        filesystem.recreate("target", b"foreign".to_vec());

        assert_eq!(
            filesystem.guarded_prune("target", stale),
            Err(GuardedError::TokenMismatch)
        );
        assert_eq!(filesystem.bytes("target"), Some(b"foreign".as_slice()));
    }

    #[test]
    fn unsupported_guarded_operations_fail_closed() {
        let capabilities = Capabilities {
            create_no_replace: true,
            guarded_replace: false,
            guarded_prune: false,
        };
        let mut filesystem = FakeFilesystem::new(capabilities);
        let token = filesystem.insert_foreign("target", b"foreign".to_vec());

        assert_eq!(
            filesystem.guarded_replace("target", token, b"desired".to_vec()),
            Err(GuardedError::Unsupported)
        );
        assert_eq!(
            filesystem.guarded_prune("target", token),
            Err(GuardedError::Unsupported)
        );
        assert_eq!(filesystem.bytes("target"), Some(b"foreign".as_slice()));
    }

    #[test]
    fn absent_modify_does_not_change_model_state() {
        let mut attempted = FakeFilesystem::new(supported());
        let mut untouched = FakeFilesystem::new(supported());

        assert_eq!(
            attempted.modify("absent", b"ignored".to_vec()),
            Err(GuardedError::Absent)
        );

        let attempted_token = attempted
            .create_no_replace("target", b"value".to_vec())
            .unwrap();
        let untouched_token = untouched
            .create_no_replace("target", b"value".to_vec())
            .unwrap();
        assert_eq!(attempted_token, untouched_token);
    }
}
