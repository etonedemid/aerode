//! The Recycle Bin: the freedesktop.org trash (home trash plus the `.Trash-$uid` folders of other
//! mounted drives). Trashing is done here rather than with g_file_trash so the exact name an item
//! got in the trash is known – that is what Undo (Ctrl+Z) and "Restore this item" need.
use std::io::Write;
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};

use gtk::glib;

/// One item in a trash directory.
#[derive(Clone, Debug, PartialEq)]
pub struct Trashed {
    /// where it was deleted from
    pub orig: PathBuf,
    /// the item inside `<trash>/files`
    pub file: PathBuf,
    /// its `<trash>/info/<name>.trashinfo`
    pub info: PathBuf,
    /// DeletionDate as a unix time (local time in the file, like the spec says)
    pub deleted: i64,
}

pub fn home_trash() -> PathBuf {
    glib::user_data_dir().join("Trash")
}

fn uid() -> u32 {
    unsafe { libc::getuid() }
}

/// Every trash directory that exists: the home trash first, then those on mounted drives.
pub fn trash_dirs() -> Vec<PathBuf> {
    let mut v = vec![home_trash()];
    if let Ok(m) = std::fs::read_to_string("/proc/self/mounts") {
        for line in m.lines() {
            let Some(mp) = line.split_whitespace().nth(1) else { continue };
            let mp = unescape_mount(mp);
            if mp == "/" || mp.starts_with("/proc") || mp.starts_with("/sys") || mp.starts_with("/dev") || mp.starts_with("/run/user") { continue; }
            for cand in [Path::new(&mp).join(format!(".Trash-{}", uid())), Path::new(&mp).join(".Trash").join(uid().to_string())] {
                if cand.join("info").is_dir() && !v.contains(&cand) { v.push(cand); }
            }
        }
    }
    v
}

fn unescape_mount(s: &str) -> String {
    // /proc/mounts escapes space, tab, newline and backslash as \ooo
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        // bytes, not str slices: a mount point can have non-ASCII characters
        if b[i] == b'\\' && i + 4 <= b.len() && b[i + 1..i + 4].iter().all(|c| (b'0'..=b'7').contains(c)) {
            out.push(((b[i + 1] - b'0') as u32 * 64 + (b[i + 2] - b'0') as u32 * 8 + (b[i + 3] - b'0') as u32) as u8);
            i += 4;
            continue;
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Percent-encode a path for the `Path=` key (RFC 2396 escaping of everything but unreserved and '/').
pub fn encode_path(p: &Path) -> String {
    use std::os::unix::ffi::OsStrExt;
    let mut s = String::new();
    for &c in p.as_os_str().as_bytes() {
        if c.is_ascii_alphanumeric() || b"/-_.~!$&'()*+,;=:@".contains(&c) { s.push(c as char); } else { s.push_str(&format!("%{c:02X}")); }
    }
    s
}

pub fn decode_path(s: &str) -> PathBuf {
    use std::os::unix::ffi::OsStringExt;
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        // other programs write UTF-8 into Path= too: bytes, not str slices
        if b[i] == b'%' && i + 2 < b.len() {
            let hex = |c: u8| (c as char).to_digit(16).map(|v| v as u8);
            if let (Some(h), Some(l)) = (hex(b[i + 1]), hex(b[i + 2])) { out.push(h * 16 + l); i += 3; continue; }
        }
        out.push(b[i]);
        i += 1;
    }
    PathBuf::from(std::ffi::OsString::from_vec(out))
}

/// The mount point (top directory) of the filesystem `p` is on.
fn topdir(p: &Path) -> Option<PathBuf> {
    let dev = std::fs::symlink_metadata(p).ok()?.dev();
    let mut cur = p.parent()?.to_path_buf();
    loop {
        let Some(parent) = cur.parent() else { return Some(cur) };
        match std::fs::metadata(parent) {
            Ok(m) if m.dev() == dev => cur = parent.to_path_buf(),
            _ => return Some(cur),
        }
    }
}

/// The trash directory to use for `p`: the home trash when it is on the same filesystem, else the
/// drive's own `$topdir/.Trash/$uid` or `$topdir/.Trash-$uid` (created if needed).
fn trash_for(p: &Path) -> std::io::Result<(PathBuf, Option<PathBuf>)> {
    let dev = std::fs::symlink_metadata(p)?.dev();
    let home = home_trash();
    let mk = |d: &Path| -> std::io::Result<()> {
        for sub in ["files", "info"] {
            let s = d.join(sub);
            if !s.is_dir() { std::fs::DirBuilder::new().recursive(true).mode(0o700).create(&s)?; }
        }
        Ok(())
    };
    // the home trash lives on the filesystem of the data dir
    let data = glib::user_data_dir();
    if std::fs::metadata(&data).map(|m| m.dev() == dev).unwrap_or(false) {
        mk(&home)?;
        return Ok((home, None));
    }
    let top = topdir(p).ok_or_else(|| std::io::Error::new(std::io::ErrorKind::Unsupported, "no top directory"))?;
    let shared = top.join(".Trash");
    if let Ok(m) = std::fs::symlink_metadata(&shared) {
        // the spec requires the sticky bit and no symlink
        if m.is_dir() && m.mode() & 0o1000 != 0 {
            let d = shared.join(uid().to_string());
            if mk(&d).is_ok() { return Ok((d, Some(top))); }
        }
    }
    let d = top.join(format!(".Trash-{}", uid()));
    if let Ok(m) = std::fs::symlink_metadata(&d) {
        if m.file_type().is_symlink() { return Err(std::io::Error::new(std::io::ErrorKind::Unsupported, "trash is a symlink")); }
    }
    mk(&d)?;
    Ok((d, Some(top)))
}

fn now_local() -> (String, i64) {
    let n = chrono::Local::now();
    (n.format("%Y-%m-%dT%H:%M:%S").to_string(), n.timestamp())
}

/// Move `p` to the trash. Errors with `Unsupported` when the drive has no usable trash
/// (Explorer then offers to delete permanently).
pub fn trash(p: &Path) -> std::io::Result<Trashed> {
    let p = if p.is_absolute() { p.to_path_buf() } else { std::env::current_dir()?.join(p) };
    let (dir, top) = trash_for(&p)?;
    let name = p.file_name().ok_or_else(|| std::io::Error::new(std::io::ErrorKind::InvalidInput, "no file name"))?.to_os_string();
    // info path for the original location: absolute for the home trash, relative to the drive otherwise
    let rel = match &top { Some(t) => p.strip_prefix(t).map(|r| r.to_path_buf()).unwrap_or_else(|_| p.clone()), None => p.clone() };
    let (date, ts) = now_local();
    let body = format!("[Trash Info]\nPath={}\nDeletionDate={date}\n", encode_path(&rel));
    let base = name.to_string_lossy().into_owned();
    let (stem, ext) = split_ext(&base);
    for n in 1..100000u32 {
        let cand: std::ffi::OsString = if n == 1 { name.clone() } else { format!("{stem}.{n}{ext}").into() };
        let info = dir.join("info").join({ let mut s = cand.clone(); s.push(".trashinfo"); s });
        match std::fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(&info) {
            Ok(mut f) => {
                if dir.join("files").join(&cand).symlink_metadata().is_ok() { let _ = std::fs::remove_file(&info); continue; }
                f.write_all(body.as_bytes())?;
                drop(f);
                let file = dir.join("files").join(&cand);
                if let Err(e) = std::fs::rename(&p, &file) {
                    let _ = std::fs::remove_file(&info);
                    // a different filesystem after all (bind mounts): no trash
                    if e.raw_os_error() == Some(libc::EXDEV) { return Err(std::io::Error::new(std::io::ErrorKind::Unsupported, e.to_string())); }
                    return Err(e);
                }
                return Ok(Trashed { orig: p, file, info, deleted: ts });
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e),
        }
    }
    Err(std::io::Error::new(std::io::ErrorKind::Other, "too many items with this name in the Recycle Bin"))
}

/// "name.txt" -> ("name", ".txt"); dotfiles and names without a dot have no extension.
pub fn split_ext(name: &str) -> (&str, &str) {
    match name.rfind('.') { Some(i) if i > 0 && i + 1 < name.len() => (&name[..i], &name[i..]), _ => (name, "") }
}

/// Parse one .trashinfo file of trash directory `dir`.
pub fn read_info(dir: &Path, info: &Path) -> Option<Trashed> {
    let text = std::fs::read_to_string(info).ok()?;
    let mut path = None;
    let mut date = 0i64;
    for l in text.lines() {
        if let Some(v) = l.strip_prefix("Path=") { path = Some(decode_path(v.trim())); }
        if let Some(v) = l.strip_prefix("DeletionDate=") {
            date = chrono::NaiveDateTime::parse_from_str(v.trim(), "%Y-%m-%dT%H:%M:%S").ok()
                .and_then(|d| d.and_local_timezone(chrono::Local).earliest()).map(|d| d.timestamp()).unwrap_or(0);
        }
    }
    let path = path?;
    // relative paths are relative to the drive the trash is on
    let orig = if path.is_absolute() { path } else {
        let top = if dir.parent().and_then(|p| p.file_name()) == Some(std::ffi::OsStr::new(".Trash")) { dir.parent()?.parent()? } else { dir.parent()? };
        top.join(path)
    };
    let file = dir.join("files").join(info.file_stem()?);
    Some(Trashed { orig, file, info: info.to_path_buf(), deleted: date })
}

/// Everything in all trash directories (items whose file vanished are skipped).
pub fn list() -> Vec<Trashed> {
    let mut v = vec![];
    for d in trash_dirs() {
        let Ok(rd) = std::fs::read_dir(d.join("info")) else { continue };
        for e in rd.flatten() {
            let p = e.path();
            if p.extension().map(|x| x != "trashinfo").unwrap_or(true) { continue; }
            if let Some(t) = read_info(&d, &p) { if t.file.symlink_metadata().is_ok() { v.push(t); } }
        }
    }
    v
}

/// The trash entry for a file inside some `<trash>/files` directory.
pub fn lookup(file: &Path) -> Option<Trashed> {
    let dir = file.parent()?.parent()?;
    let mut name = file.file_name()?.to_os_string();
    name.push(".trashinfo");
    read_info(dir, &dir.join("info").join(name))
}

pub fn is_empty() -> bool {
    trash_dirs().iter().all(|d| std::fs::read_dir(d.join("files")).map(|mut r| r.next().is_none()).unwrap_or(true))
}

/// Number of items in the Recycle Bin.
pub fn count() -> usize {
    trash_dirs().iter().map(|d| std::fs::read_dir(d.join("files")).map(|r| r.count()).unwrap_or(0)).sum()
}

/// Is `p` inside one of the trash `files` directories?
pub fn in_trash(p: &Path) -> bool {
    p.parent().map(|f| f.file_name() == Some(std::ffi::OsStr::new("files")) && f.parent().map(|t| trash_dirs().iter().any(|d| d == t)).unwrap_or(false)).unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn path_encoding_roundtrip() {
        use std::os::unix::ffi::OsStrExt;
        let p = PathBuf::from(std::ffi::OsStr::from_bytes(b"/tmp/a b/%c\xff/\xc3\xa9.txt"));
        let e = encode_path(&p);
        assert!(!e.contains(' '));
        assert_eq!(decode_path(&e), p);
    }

    #[test]
    fn ext_split() {
        assert_eq!(split_ext("a.txt"), ("a", ".txt"));
        assert_eq!(split_ext(".bashrc"), (".bashrc", ""));
        assert_eq!(split_ext("noext"), ("noext", ""));
        assert_eq!(split_ext("a.tar.gz"), ("a.tar", ".gz"));
        assert_eq!(split_ext("trailing."), ("trailing.", ""));
    }

    #[test]
    fn mount_unescape() {
        assert_eq!(unescape_mount("/run/media/u/My\\040Disk"), "/run/media/u/My Disk");
        assert_eq!(unescape_mount("/run/media/u/D\\303\\251j\\303\\240"), "/run/media/u/Déjà");
        // no panic on characters right after a backslash
        assert_eq!(unescape_mount("/m/\\é"), "/m/\\é");
    }

    #[test]
    fn decode_non_ascii() {
        // Path= written unescaped by another program, and a stray % before a multibyte character
        assert_eq!(decode_path("/tmp/été %é"), PathBuf::from("/tmp/été %é"));
        assert_eq!(decode_path("/a%2"), PathBuf::from("/a%2"));
    }
}
