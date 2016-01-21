use std::iter;

use tar::Header;

#[test]
fn default_gnu() {
    let mut h = Header::new_gnu();
    assert!(h.as_gnu().is_some());
    assert!(h.as_gnu_mut().is_some());
    assert!(h.as_ustar().is_none());
    assert!(h.as_ustar_mut().is_none());
}

#[test]
fn goto_old() {
    let mut h = Header::new_old();
    assert!(h.as_gnu().is_none());
    assert!(h.as_gnu_mut().is_none());
    assert!(h.as_ustar().is_none());
    assert!(h.as_ustar_mut().is_none());
}

#[test]
fn goto_ustar() {
    let mut h = Header::new_ustar();
    assert!(h.as_gnu().is_none());
    assert!(h.as_gnu_mut().is_none());
    assert!(h.as_ustar().is_some());
    assert!(h.as_ustar_mut().is_some());
}

#[test]
fn link_name() {
    let mut h = Header::new_gnu();
    t!(h.set_link_name("foo"));
    assert_eq!(t!(h.link_name()).unwrap().to_str(), Some("foo"));
    t!(h.set_link_name("foo/bar"));
    assert_eq!(t!(h.link_name()).unwrap().to_str(), Some("foo/bar"));
    t!(h.set_link_name("foo\\ba"));
    assert_eq!(t!(h.link_name()).unwrap().to_str(), Some("foo/ba"));

    let name = "foo\\bar\0";
    for (slot, val) in h.as_old_mut().linkname.iter_mut().zip(name.as_bytes()) {
        *slot = *val;
    }
    assert_eq!(t!(h.link_name()).unwrap().to_str(), Some("foo/bar"));

    assert!(h.set_link_name("\0").is_err());
}

#[test]
fn user_and_group_name() {
    let mut h = Header::new_gnu();
    t!(h.set_username("foo"));
    t!(h.set_groupname("bar"));
    assert_eq!(t!(h.username()), Some("foo"));
    assert_eq!(t!(h.groupname()), Some("bar"));

    h = Header::new_ustar();
    t!(h.set_username("foo"));
    t!(h.set_groupname("bar"));
    assert_eq!(t!(h.username()), Some("foo"));
    assert_eq!(t!(h.groupname()), Some("bar"));

    h = Header::new_old();
    assert_eq!(t!(h.username()), None);
    assert_eq!(t!(h.groupname()), None);
    assert!(h.set_username("foo").is_err());
    assert!(h.set_groupname("foo").is_err());
}

#[test]
fn dev_major_minor() {
    let mut h = Header::new_gnu();
    t!(h.set_device_major(1));
    t!(h.set_device_minor(2));
    assert_eq!(t!(h.device_major()), Some(1));
    assert_eq!(t!(h.device_minor()), Some(2));

    h = Header::new_ustar();
    t!(h.set_device_major(1));
    t!(h.set_device_minor(2));
    assert_eq!(t!(h.device_major()), Some(1));
    assert_eq!(t!(h.device_minor()), Some(2));

    h.as_ustar_mut().unwrap().dev_minor[0] = 0xff;
    h.as_ustar_mut().unwrap().dev_major[0] = 0xff;
    assert!(h.device_major().is_err());
    assert!(h.device_minor().is_err());

    h.as_ustar_mut().unwrap().dev_minor[0] = b'g';
    h.as_ustar_mut().unwrap().dev_major[0] = b'h';
    assert!(h.device_major().is_err());
    assert!(h.device_minor().is_err());

    h = Header::new_old();
    assert_eq!(t!(h.device_major()), None);
    assert_eq!(t!(h.device_minor()), None);
    assert!(h.set_device_major(1).is_err());
    assert!(h.set_device_minor(1).is_err());
}

#[test]
fn set_path() {
    let mut h = Header::new_gnu();
    t!(h.set_path("foo"));
    assert_eq!(t!(h.path()).to_str(), Some("foo"));
    t!(h.set_path("foo/bar"));
    assert_eq!(t!(h.path()).to_str(), Some("foo/bar"));
    t!(h.set_path("foo\\bar"));
    assert_eq!(t!(h.path()).to_str(), Some("foo/bar"));
    let name = "foo\\bar\0";
    for (slot, val) in h.as_old_mut().name.iter_mut().zip(name.as_bytes()) {
        *slot = *val;
    }
    assert_eq!(t!(h.path()).to_str(), Some("foo/bar"));

    let long_name = iter::repeat("foo").take(100).collect::<String>();
    let medium1 = iter::repeat("foo").take(52).collect::<String>();
    let medium2 = iter::repeat("fo/").take(52).collect::<String>();

    assert!(h.set_path(&long_name).is_err());
    assert!(h.set_path(&medium1).is_err());
    assert!(h.set_path(&medium2).is_err());
    assert!(h.set_path("\0").is_err());

    h = Header::new_ustar();
    t!(h.set_path("foo"));
    assert_eq!(t!(h.path()).to_str(), Some("foo"));

    assert!(h.set_path(&long_name).is_err());
    assert!(h.set_path(&medium1).is_err());
    t!(h.set_path(&medium2));
    assert_eq!(t!(h.path()).to_str(), Some(&medium2[..]));
}
