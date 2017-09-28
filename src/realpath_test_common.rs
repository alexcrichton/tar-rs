use realpath::realpath;
use std::path::{Path, PathBuf};

#[test]
fn test_err_basic() {
    realpath(Path::new(""), None).unwrap_err();
    realpath(Path::new(""), None).unwrap_err();
    realpath(Path::new(""), Some(PathBuf::from(""))).unwrap_err();
    realpath(Path::new(""), Some(PathBuf::from("/"))).unwrap_err();
}

#[test]
fn test_err_relative_base() {
    realpath(Path::new("."), None).unwrap_err();
    realpath(Path::new("."), None).unwrap_err();
    realpath(Path::new("./"), None).unwrap_err();
    realpath(Path::new("./"), None).unwrap_err();
    realpath(Path::new(".."), None).unwrap_err();
    realpath(Path::new(".."), None).unwrap_err();
    realpath(Path::new("../"), None).unwrap_err();
    realpath(Path::new("../"), None).unwrap_err();
    realpath(Path::new("."), Some(PathBuf::from("."))).unwrap_err();
    realpath(Path::new("."), Some(PathBuf::from("."))).unwrap_err();
    realpath(Path::new("."), Some(PathBuf::from("./"))).unwrap_err();
    realpath(Path::new("."), Some(PathBuf::from("./"))).unwrap_err();
    realpath(Path::new("."), Some(PathBuf::from(".."))).unwrap_err();
    realpath(Path::new("."), Some(PathBuf::from(".."))).unwrap_err();
    realpath(Path::new("."), Some(PathBuf::from("../"))).unwrap_err();
    realpath(Path::new("."), Some(PathBuf::from("../"))).unwrap_err();
}
