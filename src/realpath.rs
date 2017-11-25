/// realpath - path cleaning and links flattening, akin to `realpath -m`.

use error::*;
use std::io::{self, Error, ErrorKind};
use std::path::{Component, MAIN_SEPARATOR, Path, PathBuf};

/// Maximum number of symbolic links followed, see `path_resolution(7)`.
const LINKS_LIMIT: u8 = 40;

/// Normalize `path`.
///
/// See `std::path::Path::components()` for a full description of
/// normalization steps.
pub fn normalize<P: AsRef<Path>>(path: P) -> PathBuf {
    path.as_ref().components().as_path().to_path_buf()
}

/// Return the canonical absolute name of `path`.
///
/// This canonicalize paths, allowing non-existing path-components.
/// The final canonical name will not contain any ".", "..", or
/// repeated-separator components. All symlinks which exist at
/// the time of invocation will be resolved to their destinations.
/// An optional `base` parameter is used as anchor path if input `path`
/// is relative.
///
/// # Errors
///
/// `path` cannot be empty. If `path` is relative, `base` cannot be `None`.
/// If `allow_missing` is `false`, this will fail if any path-components
/// do not exist. Recursive symlinks are detected and bailed upon, as well
/// as overlong (>40) link-chains.
pub fn realpath<P: AsRef<Path>>(path: P, base: Option<PathBuf>) -> io::Result<PathBuf> {
    if path.as_ref().components().count() == 0 {
        return Err(
            TarError::new(
                "Empty input path",
                Error::new(ErrorKind::Other, "Invalid argument"),
            ).into(),
        );
    }

    // If relative, anchor input to base.
    let path = match path.as_ref().has_root() {
        false => base.unwrap_or_default().join(path),
        true => path.as_ref().to_path_buf(),
    };
    if !path.has_root() {
        return Err(
            TarError::new(
                &format!("Relative base/path {}", &path.display()),
                Error::new(ErrorKind::Other, "Invalid argument"),
            ).into(),
        );
    }

    // Normalize any double-separator and dot-dirs.
    let path = normalize(path);

    // Resolve links and dot-dot-dirs.
    let path = try!(resolve(&path, &PathBuf::new(), LINKS_LIMIT));

    // Ensure final result is meaningful.
    if path.components().count() == 0 {
        return Err(
            TarError::new(
                "Empty resolved path",
                Error::new(ErrorKind::Other, "Invalid argument"),
            ).into(),
        );
    };
    Ok(path)
}

/// Symlink resolution and path cleanup.
///
/// This resolves `path` to a clean absolute path. Components are processed
/// and resolved starting from the top level (root). A symlink at any point
/// induces a recursion step to clean up the new target and then continue
/// with the remaining components.
fn resolve<P: AsRef<Path>>(path: P, base: P, limit: u8) -> io::Result<PathBuf> {
    let mut resolved = PathBuf::new();

    // Limit recursion depth. Aborting here is equivalent to -ELOOP.
    if limit == 0 {
        return Err(
            TarError::new(
                &format!("Links recursion limit ({}) reached", LINKS_LIMIT),
                Error::new(ErrorKind::Other, "Too many symbolic links"),
            ).into(),
        );
    }

    // Join base+path, *without* resetting base if path is absolute.
    let full = base.as_ref()
        .components()
        .chain(path.as_ref().components())
        .fold(PathBuf::new(), |r, p| r.join(p.as_os_str()));

    let chained = full.components();
    for (i, c) in chained.clone().enumerate() {
        match c {
            // Preserve Windows prefix.
            Component::Prefix(p) => {
                resolved.push(p.as_os_str());
            }
            // Reset current resolved result.
            Component::RootDir => {
                resolved.push(PathBuf::from(MAIN_SEPARATOR.to_string()));
            }
            // Skip dot-dir.
            Component::CurDir => {}
            // Skip dot-dot-dir. This is safe here because we already resolved
            // any existing symlinks in parents.
            Component::ParentDir => {
                resolved.pop();
            }
            // Append nominal path components. In case of symlink, dereference it
            // and restart resolution process, as the link target could be in any
            // non-resolved form.
            Component::Normal(p) => {
                // Peek ahead to check whether there is a symlink to process.
                let cur = resolved.join(&p);
                let target = cur.symlink_metadata().and_then(|_| cur.read_link());
                match target {
                    Ok(t) => resolved.push(t),
                    _ => {
                        // Not a symlink. Append component and proceed.
                        resolved.push(p);
                        continue;
                    }
                };

                // Symlink encountered. Re-group remaining components and recurse
                // to validate both the new target (in `resolved`) and those leftovers.
                let remaining = chained.skip(i + 1).fold(PathBuf::new(), |r, p| {
                    r.join(p.as_os_str())
                });
                resolved = try!(resolve(&remaining, &resolved, limit - 1));
                break;
            }
        }
    }

    // Normalize any spurious double-separator and dot-dirs.
    let path = normalize(resolved);
    Ok(path)
}
