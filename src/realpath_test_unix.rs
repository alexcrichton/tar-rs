extern crate tempdir;

use realpath::realpath;
use std::path::{Path, PathBuf};
use self::tempdir::TempDir;
use std::os::unix::fs::symlink;

#[test]
fn test_ok_basic() {
    assert_eq!(realpath(Path::new("/"), None).unwrap(), PathBuf::from("/"));
    assert_eq!(realpath(Path::new("."), Some(PathBuf::from("/"))).unwrap(), PathBuf::from("/"));
    assert_eq!(realpath(Path::new(".."), Some(PathBuf::from("/"))).unwrap(), PathBuf::from("/"));
    assert_eq!(realpath(Path::new("../.."), Some(PathBuf::from("/"))).unwrap(), PathBuf::from("/"));
    assert_eq!(realpath(Path::new("/root"), None).unwrap(), PathBuf::from("/root"));
    assert_eq!(realpath(Path::new("/foobar"), None).unwrap(), PathBuf::from("/foobar"));
}

#[test]
fn test_ok_canonicalize() {
    assert_eq!(realpath(Path::new("/bin"), None).unwrap(), PathBuf::from("/bin"));
    assert_eq!(realpath(Path::new("/bin"), Some(PathBuf::from("./foo"))).unwrap(), PathBuf::from("/bin"));
    assert_eq!(realpath(Path::new("../../bin"), Some(PathBuf::from("/usr/share"))).unwrap(), PathBuf::from("/bin"));
    assert_eq!(realpath(Path::new("../../bin"), Some(PathBuf::from("/"))).unwrap(), PathBuf::from("/bin"));
    assert_eq!(realpath(Path::new("."), Some(PathBuf::from("/bin"))).unwrap(), PathBuf::from("/bin"));
    assert_eq!(realpath(Path::new(".."), Some(PathBuf::from("/usr/bin"))).unwrap(), PathBuf::from("/usr"));
}

#[test]
fn test_ok_resolve() {
    assert_eq!(realpath(Path::new("/foo"), None).unwrap(), PathBuf::from("/foo"));
    assert_eq!(realpath(Path::new("/foo/."), None).unwrap(), PathBuf::from("/foo"));
    assert_eq!(realpath(Path::new("/foo/.."), None).unwrap(), PathBuf::from("/"));
    assert_eq!(realpath(Path::new("/foo/../.."), None).unwrap(), PathBuf::from("/"));
    assert_eq!(realpath(Path::new("/foo/./.."), None).unwrap(), PathBuf::from("/"));
    assert_eq!(realpath(Path::new("/foo/../."), None).unwrap(), PathBuf::from("/"));
    assert_eq!(realpath(Path::new("/foo/../bar/.."), None).unwrap(), PathBuf::from("/"));
    assert_eq!(realpath(Path::new("/foo/../bar/../foo"), None).unwrap(), PathBuf::from("/foo"));
    assert_eq!(realpath(Path::new("/foo/bar/.."), None).unwrap(), PathBuf::from("/foo"));
    assert_eq!(realpath(Path::new("/foo"), Some(PathBuf::from("./foo"))).unwrap(), PathBuf::from("/foo"));
    assert_eq!(realpath(Path::new("../../foo"), Some(PathBuf::from("/usr/share"))).unwrap(), PathBuf::from("/foo"));
    assert_eq!(realpath(Path::new("../../foo"), Some(PathBuf::from("/"))).unwrap(), PathBuf::from("/foo"));
    assert_eq!(realpath(Path::new("."), Some(PathBuf::from("/foo"))).unwrap(), PathBuf::from("/foo"));
    assert_eq!(realpath(Path::new(".."), Some(PathBuf::from("/usr/foo"))).unwrap(), PathBuf::from("/usr"));
}

#[test]
fn test_ok_basic_symlink() {
    let t1 = TempDir::new("ok_symlink").unwrap();
    let src = t1.path().join("src");
    let dst = t1.path().join("dst");
    symlink(&src, &dst).unwrap();
    assert_eq!(realpath(&dst, None).unwrap(), src);
    drop(t1);
    assert_eq!(realpath(&dst, None).unwrap(), dst);
}

#[test]
fn test_err_recursive_symlink() {
    let t1 = TempDir::new("err_rec_symlink").unwrap();
    let src = t1.path().join("src");
    symlink(&src, &src).unwrap();
    realpath(&src, None).unwrap_err();
    drop(t1);
    assert_eq!(realpath(&src, None).unwrap(), src);
}

#[test]
fn test_ok_relative_symlink() {
    let t1 = TempDir::new("ok_rel_symlink").unwrap();
    let src = PathBuf::from(".");
    let dst = t1.path().join("dst");
    symlink(&src, &dst).unwrap();
    assert_eq!(realpath(&dst, None).unwrap(), t1.path());
    drop(t1);
    assert_eq!(realpath(&dst, None).unwrap(), dst);
}

#[test]
fn test_ok_root_symlink() {
    let t1 = TempDir::new("ok_root_symlink").unwrap();
    let src = PathBuf::from("/");
    let dst = t1.path().join("dst");
    symlink(&src, &dst).unwrap();
    assert_eq!(realpath(&dst, None).unwrap(), src);
    drop(t1);
    assert_eq!(realpath(&dst, None).unwrap(), dst);
}

#[test]
fn test_err_root_symlink() {
    let t1 = TempDir::new("err_root_symlink").unwrap();
    let src = PathBuf::from("/");
    let dst = t1.path().join("dst");
    symlink(&src, &dst).unwrap();
    assert_eq!(realpath(&dst, None).unwrap(), src);
}

#[test]
fn test_ok_relative_components_symlink() {
    let t1 = TempDir::new("ok_relcomp_symlink").unwrap();
    let src = t1.path().join("foo").join("bar").join("..").join("..").join(".");
    let dst = t1.path().join("dst");
    symlink(&src, &dst).unwrap();
    assert_eq!(realpath(&dst, None).unwrap(), t1.path());
}
