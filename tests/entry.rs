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
fn absolute_symlink() {
    let mut ar = tar::Builder::new(Vec::new());

    let mut header = tar::Header::new_gnu();
    header.set_size(0);
    header.set_entry_type(tar::EntryType::Symlink);
    t!(header.set_path("foo"));
    t!(header.set_link_name("/bar"));
    header.set_cksum();
    t!(ar.append(&header, &[][..]));

    let bytes = t!(ar.into_inner());
    let mut ar = tar::Archive::new(&bytes[..]);

    let td = t!(TempDir::new("tar"));
    t!(ar.unpack(td.path()));

    t!(td.path().join("foo").symlink_metadata());

    let mut ar = tar::Archive::new(&bytes[..]);
    let mut entries = t!(ar.entries());
    let entry = t!(entries.next().unwrap());
    assert_eq!(&*entry.link_name_bytes().unwrap(), b"/bar");
}

#[test]
fn absolute_hardlink() {
    let td = t!(TempDir::new("tar"));
    let mut ar = tar::Builder::new(Vec::new());

    let mut header = tar::Header::new_gnu();
    header.set_size(0);
    header.set_entry_type(tar::EntryType::Regular);
    t!(header.set_path("foo"));
    header.set_cksum();
    t!(ar.append(&header, &[][..]));

    let mut header = tar::Header::new_gnu();
    header.set_size(0);
    header.set_entry_type(tar::EntryType::Link);
    t!(header.set_path("bar"));
    // This absolute path under tempdir will be created at unpack time
    t!(header.set_link_name(td.path().join("foo")));
    header.set_cksum();
    t!(ar.append(&header, &[][..]));

    let bytes = t!(ar.into_inner());
    let mut ar = tar::Archive::new(&bytes[..]);

    t!(ar.unpack(td.path()));
    t!(td.path().join("foo").metadata());
    t!(td.path().join("bar").metadata());
}

#[test]
fn relative_hardlink() {
    let mut ar = tar::Builder::new(Vec::new());

    let mut header = tar::Header::new_gnu();
    header.set_size(0);
    header.set_entry_type(tar::EntryType::Regular);
    t!(header.set_path("foo"));
    header.set_cksum();
    t!(ar.append(&header, &[][..]));

    let mut header = tar::Header::new_gnu();
    header.set_size(0);
    header.set_entry_type(tar::EntryType::Link);
    t!(header.set_path("bar"));
    t!(header.set_link_name("foo"));
    header.set_cksum();
    t!(ar.append(&header, &[][..]));

    let bytes = t!(ar.into_inner());
    let mut ar = tar::Archive::new(&bytes[..]);

    let td = t!(TempDir::new("tar"));
    t!(ar.unpack(td.path()));
    t!(td.path().join("foo").metadata());
    t!(td.path().join("bar").metadata());
}

#[test]
fn absolute_link_deref_error() {
    let mut ar = tar::Builder::new(Vec::new());

    let mut header = tar::Header::new_gnu();
    header.set_size(0);
    header.set_entry_type(tar::EntryType::Symlink);
    t!(header.set_path("foo"));
    t!(header.set_link_name("/"));
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
    assert!(ar.unpack(td.path()).is_err());
    t!(td.path().join("foo").symlink_metadata());
    assert!(File::open(td.path().join("foo").join("bar")).is_err());
}

#[test]
fn chase_absolute_link_error() {
    let outside = t!(TempDir::new("outside"));
    let mut ar = tar::Builder::new(Vec::new());

    let mut header = tar::Header::new_gnu();
    header.set_size(0);
    header.set_entry_type(tar::EntryType::Symlink);
    t!(header.set_path("symlink"));
    t!(header.set_link_name(outside.path().join("unreachable")));
    header.set_cksum();
    t!(ar.append(&header, &[][..]));

    let mut header = tar::Header::new_gnu();
    header.set_size(0);
    header.set_entry_type(tar::EntryType::Regular);
    t!(header.set_path("symlink"));
    header.set_cksum();
    t!(ar.append(&header, &[][..]));

    let bytes = t!(ar.into_inner());
    let mut ar = tar::Archive::new(&bytes[..]);

    let td = t!(TempDir::new("tar"));
    assert!(ar.unpack(td.path()).is_err());
    t!(td.path().join("symlink").symlink_metadata());
    assert!(File::open(outside.path().join("unreachable")).is_err());
}

#[test]
fn chase_relative_link_error() {
    use std::path::PathBuf;
    let outside = t!(TempDir::new("outside"));
    let mut ar = tar::Builder::new(Vec::new());

    let mut header = tar::Header::new_gnu();
    header.set_size(0);
    header.set_entry_type(tar::EntryType::Symlink);
    t!(header.set_path("symlink"));
    let target = outside.path().components()
        .fold(PathBuf::new(), |acc, _| acc.join("../"))
        .components()
        .chain(outside.path().join("unreachable").components())
        .fold(PathBuf::new(), |acc, p| {
            match p == ::std::path::Component::RootDir {
                false => acc.join(p.as_os_str()),
                true => acc,
            }
        });

    t!(header.set_link_name(target));
    header.set_cksum();
    t!(ar.append(&header, &[][..]));

    let mut header = tar::Header::new_gnu();
    header.set_size(0);
    header.set_entry_type(tar::EntryType::Regular);
    t!(header.set_path("symlink"));
    header.set_cksum();
    t!(ar.append(&header, &[][..]));

    let bytes = t!(ar.into_inner());
    let mut ar = tar::Archive::new(&bytes[..]);

    let td = t!(TempDir::new("tar_chase_relative"));
    ar.unpack(td.path()).unwrap_err();
    t!(td.path().join("symlink").symlink_metadata());
    File::open(outside.path().join("unreachable")).unwrap_err();
}

#[test]
fn relative_link_deref_error() {
    let mut ar = tar::Builder::new(Vec::new());

    let mut header = tar::Header::new_gnu();
    header.set_size(0);
    header.set_entry_type(tar::EntryType::Symlink);
    t!(header.set_path("foo"));
    t!(header.set_link_name("../../../../"));
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
    assert!(ar.unpack(td.path()).is_err());
    t!(td.path().join("foo").symlink_metadata());
    assert!(File::open(td.path().join("foo").join("bar")).is_err());
}

#[test]
fn modify_link_just_created() {
    let mut ar = tar::Builder::new(Vec::new());

    let mut header = tar::Header::new_gnu();
    header.set_size(0);
    header.set_entry_type(tar::EntryType::Symlink);
    t!(header.set_path("foo"));
    t!(header.set_link_name("bar"));
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
fn parent_paths_error() {
    let mut ar = tar::Builder::new(Vec::new());

    let mut header = tar::Header::new_gnu();
    header.set_size(0);
    header.set_entry_type(tar::EntryType::Symlink);
    t!(header.set_path("foo"));
    t!(header.set_link_name(".."));
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
    assert!(ar.unpack(td.path()).is_err());
    t!(td.path().join("foo").symlink_metadata());
    assert!(File::open(td.path().join("foo").join("bar")).is_err());
}

#[test]
#[cfg(unix)]
fn good_parent_paths_ok() {
    use std::path::PathBuf;
    let mut ar = tar::Builder::new(Vec::new());

    let mut header = tar::Header::new_gnu();
    header.set_size(0);
    header.set_entry_type(tar::EntryType::Symlink);
    t!(header.set_path(PathBuf::from("foo").join("bar")));
    t!(header.set_link_name(PathBuf::from("..").join("bar")));
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
    t!(td.path().join("foo").join("bar").read_link());
    let dst = t!(td.path().join("foo").join("bar").canonicalize());
    t!(File::open(dst));
}
