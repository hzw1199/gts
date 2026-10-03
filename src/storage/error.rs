use std::fmt;

#[derive(Debug)]
pub struct StorageError {
    message: String,
}

impl StorageError {
    pub fn message(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }

    pub fn io(err: std::io::Error) -> Self {
        Self {
            message: err.to_string(),
        }
    }
}

impl fmt::Display for StorageError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for StorageError {}

impl From<std::io::Error> for StorageError {
    fn from(value: std::io::Error) -> Self {
        Self::io(value)
    }
}
