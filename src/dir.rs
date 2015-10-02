use std::path::{Path, PathBuf};
use std::fs::metadata;

pub trait IsDir {
    fn is_dir(&self) -> bool;
}

impl IsDir for Path {
    fn is_dir(&self) -> bool {
        metadata(self)
            .map(|s| s.is_dir())
            .unwrap_or(false)
    }
}

impl IsDir for PathBuf {
    fn is_dir(&self) -> bool {
        self.as_path().is_dir()
    }
}
