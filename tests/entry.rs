extern crate tar;
extern crate tempdir;

use std::fs::File;

use tempdir::TempDir;

macro_rules! t {
    ($e:expr) => (match $e {
        Ok(v) => v,
        Err(e) => panic!("{} returned {}", stringify!($e), e),
    })
}

#[test]
fn absolute_link_ignored() {
    let mut ar = tar::Builder::new(Vec::new());

    let mut header = tar::Header::new_gnu();
    header.set_size(0);
    header.set_entry_type(tar::EntryType::Symlink);
    t!(header.set_path("foo"));
    assert!(header.set_link_name("/bar").is_err());
    let link_name = b"/bar\0";
    header.as_gnu_mut().unwrap().linkname[..link_name.len()].copy_from_slice(link_name);
    header.set_cksum();
    t!(ar.append(&header, &[][..]));

    let mut header = tar::Header::new_gnu();
    header.set_size(0);
    header.set_entry_type(tar::EntryType::Regular);
    t!(header.set_path("bar"));
    header.set_cksum();
    t!(ar.append(&header, &[][..]));

    let bytes = t!(ar.into_inner());
    let mut ar = tar::Archive::new(&bytes[..]);

    let td = t!(TempDir::new("tar"));
    t!(ar.unpack(td.path()));

    t!(File::open(td.path().join("bar")));
    t!(File::open(td.path().join("foo")));
}

#[test]
fn modify_link_just_created() {
    let mut ar = tar::Builder::new(Vec::new());

    let mut header = tar::Header::new_gnu();
    header.set_size(0);
    header.set_entry_type(tar::EntryType::Symlink);
    t!(header.set_path("foo"));
    assert!(header.set_link_name("/bar").is_err());
    let link_name = b"/bar\0";
    header.as_gnu_mut().unwrap().linkname[..link_name.len()].copy_from_slice(link_name);
    header.set_cksum();
    t!(ar.append(&header, &[][..]));

    let mut header = tar::Header::new_gnu();
    header.set_size(0);
    header.set_entry_type(tar::EntryType::Regular);
    t!(header.set_path("bar/foo"));
    header.set_cksum();
    t!(ar.append(&header, &[][..]));

    let mut header = tar::Header::new_gnu();
    header.set_size(0);
    header.set_entry_type(tar::EntryType::Regular);
    t!(header.set_path("foo/bar"));
    header.set_cksum();
    t!(ar.append(&header, &[][..]));

    let bytes = t!(ar.into_inner());
    let mut ar = tar::Archive::new(&bytes[..]);

    let td = t!(TempDir::new("tar"));
    t!(ar.unpack(td.path()));

    t!(File::open(td.path().join("bar/foo")));
    t!(File::open(td.path().join("bar/bar")));
    t!(File::open(td.path().join("foo/foo")));
    t!(File::open(td.path().join("foo/bar")));
}

#[test]
fn parent_paths_ignored() {
    let mut ar = tar::Builder::new(Vec::new());

    let mut header = tar::Header::new_gnu();
    header.set_size(0);
    header.set_entry_type(tar::EntryType::Symlink);
    t!(header.set_path("foo"));
    assert!(header.set_link_name("/bar").is_err());
    let link_name = b"../bar\0";
    header.as_gnu_mut().unwrap().linkname[..link_name.len()].copy_from_slice(link_name);
    header.set_cksum();
    t!(ar.append(&header, &[][..]));

    let bytes = t!(ar.into_inner());
    let mut ar = tar::Archive::new(&bytes[..]);

    let td = t!(TempDir::new("tar"));
    assert!(ar.unpack(td.path()).is_err());
    assert!(td.path().join("foo").symlink_metadata().is_err());
}

#[test]
fn good_parent_paths_ok() {
    let mut ar = tar::Builder::new(Vec::new());

    let mut header = tar::Header::new_gnu();
    header.set_size(0);
    header.set_entry_type(tar::EntryType::Symlink);
    t!(header.set_path("foo/bar"));
    let link_name = b"../bar\0";
    header.as_gnu_mut().unwrap().linkname[..link_name.len()].copy_from_slice(link_name);
    header.set_cksum();
    t!(ar.append(&header, &[][..]));

    let mut header = tar::Header::new_gnu();
    header.set_size(0);
    header.set_entry_type(tar::EntryType::Regular);
    t!(header.set_path("bar"));
    header.set_cksum();
    t!(ar.append(&header, &[][..]));

    let bytes = t!(ar.into_inner());
    let mut ar = tar::Archive::new(&bytes[..]);

    let td = t!(TempDir::new("tar"));
    t!(ar.unpack(td.path()));
    t!(File::open(td.path().join("foo/bar")));
}
