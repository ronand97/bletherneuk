//! Opening docs, outputs and links safely (design §20.5).
//!
//! A file opens with `open -b <bundle ID> -- <path>`, where the bundle ID is the default
//! app for it according to LaunchServices. If the file were swapped for an app after the
//! checks, the editor or browser would be handed it as a document rather than macOS
//! launching it. Terminals, script runners and installers are refused as handlers.

use std::ffi::{CStr, c_char, c_void};
use std::os::unix::fs::MetadataExt;
use std::path::Path;

use crate::config::{Config, DANGEROUS_EXTENSIONS};

/// Default apps that would run a file rather than show it.
const REFUSED_HANDLERS: &[&str] = &[
    "com.apple.Terminal",
    "com.googlecode.iterm2",
    "com.apple.ScriptEditor2",
    "com.apple.ScriptEditor",
    "org.python.PythonLauncher",
    "com.apple.installer",
    "com.apple.Automator",
    "com.apple.automator.runner",
    "com.apple.AutomatorRunner",
];

pub fn is_refused_handler(bundle_id: &str) -> bool {
    REFUSED_HANDLERS
        .iter()
        .any(|r| r.eq_ignore_ascii_case(bundle_id))
        || bundle_id
            .to_ascii_lowercase()
            .starts_with("com.apple.automator")
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum OpenError {
    #[error("the file is missing")]
    Missing,
    #[error("not a regular file")]
    NotRegular,
    #[error("the file belongs to another user")]
    NotOwned,
    #[error("files of this type can only be shown in Finder")]
    ExtensionNotAllowed,
    #[error("no app is set to open this kind of file")]
    NoHandler,
    #[error("refusing to open with {0}, which would run the file")]
    RefusedHandler(String),
    #[error("only http and https links can be opened")]
    BadUrl,
    #[error("could not run open: {0}")]
    Spawn(String),
}

/// Runs `/usr/bin/open`. Tests swap in a recorder.
pub trait Runner {
    fn open(&self, args: &[String]) -> Result<(), OpenError>;
}

pub struct SystemOpen;

impl Runner for SystemOpen {
    fn open(&self, args: &[String]) -> Result<(), OpenError> {
        let status = std::process::Command::new("/usr/bin/open")
            .args(args)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map_err(|e| OpenError::Spawn(e.to_string()))?;
        if status.success() {
            Ok(())
        } else {
            Err(OpenError::Spawn(format!(
                "exit {}",
                status.code().unwrap_or(-1)
            )))
        }
    }
}

/// Finds default apps. Tests swap in a fixed answer.
pub trait Handlers {
    fn for_file(&self, path: &Path) -> Option<String>;
    fn browser(&self) -> Option<String>;
}

pub struct LaunchServices;

impl Handlers for LaunchServices {
    fn for_file(&self, path: &Path) -> Option<String> {
        default_app_for_file(path)
    }
    fn browser(&self) -> Option<String> {
        default_browser()
    }
}

/// True when `ext` may be opened: a doc (`md`) or a configured output type, and never
/// one of the dangerous types.
pub fn extension_allowed(ext: &str, config: &Config) -> bool {
    let ext = ext.to_ascii_lowercase();
    !DANGEROUS_EXTENSIONS.contains(&ext.as_str())
        && (ext == "md" || config.is_output_extension(&ext))
}

/// The checks made just before opening: a regular file (not a symlink), owned by the
/// user, whose real extension is still allowed.
pub fn check_file(path: &Path, config: &Config) -> Result<(), OpenError> {
    let meta = std::fs::symlink_metadata(path).map_err(|_| OpenError::Missing)?;
    if !meta.file_type().is_file() {
        return Err(OpenError::NotRegular);
    }
    if meta.uid() != unsafe { libc::getuid() } {
        return Err(OpenError::NotOwned);
    }
    let ext = crate::files::extension(path).unwrap_or_default();
    if !extension_allowed(&ext, config) {
        return Err(OpenError::ExtensionNotAllowed);
    }
    Ok(())
}

pub fn open_file(
    path: &Path,
    config: &Config,
    handlers: &dyn Handlers,
    runner: &dyn Runner,
) -> Result<(), OpenError> {
    check_file(path, config)?;
    let bundle = handlers.for_file(path).ok_or(OpenError::NoHandler)?;
    if is_refused_handler(&bundle) {
        return Err(OpenError::RefusedHandler(bundle));
    }
    runner.open(&[
        "-b".into(),
        bundle,
        "--".into(),
        path.to_string_lossy().into_owned(),
    ])
}

/// Shows a file in Finder without opening it.
pub fn reveal(path: &Path, runner: &dyn Runner) -> Result<(), OpenError> {
    let meta = std::fs::symlink_metadata(path).map_err(|_| OpenError::Missing)?;
    if meta.uid() != unsafe { libc::getuid() } {
        return Err(OpenError::NotOwned);
    }
    runner.open(&[
        "-R".into(),
        "--".into(),
        path.to_string_lossy().into_owned(),
    ])
}

pub fn open_link(url: &str, handlers: &dyn Handlers, runner: &dyn Runner) -> Result<(), OpenError> {
    let url = crate::links::validate(url).map_err(|_| OpenError::BadUrl)?;
    let bundle = handlers.browser().ok_or(OpenError::NoHandler)?;
    if is_refused_handler(&bundle) {
        return Err(OpenError::RefusedHandler(bundle));
    }
    runner.open(&["-b".into(), bundle, "--".into(), url.to_string()])
}

// LaunchServices through CoreFoundation's C API.

type CFTypeRef = *const c_void;
const UTF8: u32 = 0x0800_0100;
const ROLES_ALL: u32 = 0xFFFF_FFFF;

#[link(name = "CoreServices", kind = "framework")]
unsafe extern "C" {
    fn LSCopyDefaultApplicationURLForURL(
        url: CFTypeRef,
        roles: u32,
        error: *mut CFTypeRef,
    ) -> CFTypeRef;
}

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    fn CFURLCreateFromFileSystemRepresentation(
        alloc: CFTypeRef,
        buf: *const u8,
        len: isize,
        is_dir: u8,
    ) -> CFTypeRef;
    fn CFURLCreateWithBytes(
        alloc: CFTypeRef,
        bytes: *const u8,
        len: isize,
        encoding: u32,
        base: CFTypeRef,
    ) -> CFTypeRef;
    fn CFBundleCreate(alloc: CFTypeRef, url: CFTypeRef) -> CFTypeRef;
    fn CFBundleGetIdentifier(bundle: CFTypeRef) -> CFTypeRef;
    fn CFStringGetCString(s: CFTypeRef, buf: *mut c_char, size: isize, encoding: u32) -> u8;
    fn CFRelease(cf: CFTypeRef);
}

/// Releases a CoreFoundation object when dropped.
struct Owned(CFTypeRef);
impl Drop for Owned {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe { CFRelease(self.0) };
        }
    }
}

fn bundle_id_for_url(url: CFTypeRef) -> Option<String> {
    if url.is_null() {
        return None;
    }
    unsafe {
        let mut err: CFTypeRef = std::ptr::null();
        let app = Owned(LSCopyDefaultApplicationURLForURL(url, ROLES_ALL, &mut err));
        let _err = Owned(err);
        if app.0.is_null() {
            return None;
        }
        let bundle = Owned(CFBundleCreate(std::ptr::null(), app.0));
        if bundle.0.is_null() {
            return None;
        }
        let id = CFBundleGetIdentifier(bundle.0); // not owned
        if id.is_null() {
            return None;
        }
        let mut buf = [0 as c_char; 512];
        if CFStringGetCString(id, buf.as_mut_ptr(), buf.len() as isize, UTF8) == 0 {
            return None;
        }
        Some(CStr::from_ptr(buf.as_ptr()).to_string_lossy().into_owned())
    }
}

pub fn default_app_for_file(path: &Path) -> Option<String> {
    let bytes = path.as_os_str().as_encoded_bytes();
    let url = Owned(unsafe {
        CFURLCreateFromFileSystemRepresentation(
            std::ptr::null(),
            bytes.as_ptr(),
            bytes.len() as isize,
            0,
        )
    });
    bundle_id_for_url(url.0)
}

pub fn default_browser() -> Option<String> {
    let probe = b"https://example.com/";
    let url = Owned(unsafe {
        CFURLCreateWithBytes(
            std::ptr::null(),
            probe.as_ptr(),
            probe.len() as isize,
            UTF8,
            std::ptr::null(),
        )
    });
    bundle_id_for_url(url.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    #[derive(Default)]
    struct Recorder(RefCell<Vec<Vec<String>>>);
    impl Runner for Recorder {
        fn open(&self, args: &[String]) -> Result<(), OpenError> {
            self.0.borrow_mut().push(args.to_vec());
            Ok(())
        }
    }

    struct Fixed(&'static str);
    impl Handlers for Fixed {
        fn for_file(&self, _: &Path) -> Option<String> {
            Some(self.0.into())
        }
        fn browser(&self) -> Option<String> {
            Some(self.0.into())
        }
    }

    fn config() -> Config {
        let mut c = Config::default();
        c.outputs.extensions = vec!["html".into(), "csv".into()];
        c
    }

    #[test]
    fn opens_a_regular_file_with_its_default_app_as_a_document() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("report.html");
        std::fs::write(&p, "<html>").unwrap();
        let r = Recorder::default();
        open_file(&p, &config(), &Fixed("com.apple.Safari"), &r).unwrap();
        assert_eq!(
            r.0.borrow()[0],
            vec!["-b", "com.apple.Safari", "--", p.to_str().unwrap()]
        );
    }

    #[test]
    fn refuses_symlinks_bad_extensions_and_terminal_handlers() {
        let dir = tempfile::tempdir().unwrap();
        let real = dir.path().join("real.html");
        std::fs::write(&real, "x").unwrap();
        let link = dir.path().join("report.html");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        let r = Recorder::default();
        let h = Fixed("com.apple.Safari");
        assert_eq!(
            open_file(&link, &config(), &h, &r),
            Err(OpenError::NotRegular)
        );

        let sh = dir.path().join("run.command");
        std::fs::write(&sh, "x").unwrap();
        assert_eq!(
            open_file(&sh, &config(), &h, &r),
            Err(OpenError::ExtensionNotAllowed)
        );
        let txt = dir.path().join("notes.txt");
        std::fs::write(&txt, "x").unwrap();
        assert_eq!(
            open_file(&txt, &config(), &h, &r),
            Err(OpenError::ExtensionNotAllowed)
        );

        assert_eq!(
            open_file(&real, &config(), &Fixed("com.apple.Terminal"), &r),
            Err(OpenError::RefusedHandler("com.apple.Terminal".into()))
        );
        assert_eq!(
            open_file(&dir.path().join("gone.csv"), &config(), &h, &r),
            Err(OpenError::Missing)
        );
        assert!(r.0.borrow().is_empty());
    }

    #[test]
    fn refused_handler_list() {
        for b in [
            "com.apple.Terminal",
            "com.googlecode.iterm2",
            "com.apple.ScriptEditor2",
            "org.python.PythonLauncher",
            "com.apple.installer",
            "com.apple.Automator",
            "com.apple.automator.xpc.runner",
        ] {
            assert!(is_refused_handler(b), "{b}");
        }
        assert!(!is_refused_handler("com.apple.Safari"));
        assert!(!is_refused_handler("com.microsoft.VSCode"));
    }

    #[test]
    fn links_must_be_http_and_open_in_the_browser() {
        let r = Recorder::default();
        let h = Fixed("com.apple.Safari");
        for bad in [
            "javascript:alert(1)",
            "file:///etc/passwd",
            "smb://server/share",
            "vscode://file/x",
        ] {
            assert_eq!(open_link(bad, &h, &r), Err(OpenError::BadUrl), "{bad}");
        }
        open_link("https://example.com/a", &h, &r).unwrap();
        assert_eq!(
            r.0.borrow()[0],
            vec!["-b", "com.apple.Safari", "--", "https://example.com/a"]
        );
    }

    #[test]
    fn reveal_uses_finder() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("data.bin");
        std::fs::write(&p, "x").unwrap();
        let r = Recorder::default();
        reveal(&p, &r).unwrap();
        assert_eq!(r.0.borrow()[0], vec!["-R", "--", p.to_str().unwrap()]);
    }

    #[test]
    fn launch_services_finds_a_handler_for_plain_text() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("notes.txt");
        std::fs::write(&p, "hello").unwrap();
        let id = default_app_for_file(&p).expect("every Mac can open .txt");
        assert!(id.contains('.'), "{id}");
    }
}
