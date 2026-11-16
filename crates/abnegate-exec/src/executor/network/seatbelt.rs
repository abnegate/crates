use std::ffi::CStr;
use std::ffi::c_char;
use std::ffi::c_int;
use std::ffi::c_void;
use std::io;
use std::ptr;
use std::sync::OnceLock;

use tokio::process::Command;

use super::descriptors;

/// The seatbelt profile a denied command runs under: everything the host
/// allows, except any network at all, and asking another program to act for
/// it through LaunchServices or an Apple event.
pub(super) const PROFILE: &CStr =
    c"(version 1)(allow default)(deny network*)(deny lsopen)(deny appleevent-send)";

/// libsandbox's `struct sandbox_profile`: a compiled profile's bytecode, or
/// the name of a built-in one.
#[repr(C)]
struct Profile {
    builtin: *mut c_char,
    data: *const u8,
    size: usize,
}

#[allow(unsafe_code)]
#[link(name = "sandbox")]
unsafe extern "C" {
    fn sandbox_compile_string(
        source: *const c_char,
        parameters: *mut c_void,
        error: *mut *mut c_char,
    ) -> *mut Profile;
    fn sandbox_apply(profile: *mut Profile) -> c_int;
    fn sandbox_free_profile(profile: *mut Profile);
}

#[allow(unsafe_code)]
unsafe extern "C" {
    fn sandbox_free_error(error: *mut c_char);
}

/// Put [`PROFILE`] on `command`'s process just before it executes, which
/// every process it starts inherits and none can lift, once no descriptor
/// beyond its standard streams is left for the program to inherit. The
/// program is then executed as it would be without the profile, so a program
/// missing from the child's own `PATH` fails the spawn with
/// [`io::ErrorKind::NotFound`]. A profile that cannot be compiled or applied
/// fails the spawn too.
pub(super) fn deny(command: &mut Command) {
    let bytecode = bytecode();
    // Runs between fork and exec, so it must not allocate or take a lock
    // another thread of the parent may hold: the profile is compiled before
    // the fork, descriptors are listed into the stack, and the `dlsym` lookup
    // `sandbox_apply` makes to notify libxpc is safe because libSystem resets
    // dyld's locks in the child.
    #[allow(unsafe_code)]
    unsafe {
        command.pre_exec(move || {
            let Some(bytecode) = bytecode else {
                return Err(io::Error::from(io::ErrorKind::Unsupported));
            };
            descriptors::close_on_exec()?;
            let mut profile = Profile {
                builtin: ptr::null_mut(),
                data: bytecode.as_ptr(),
                size: bytecode.len(),
            };
            if sandbox_apply(&raw mut profile) != 0 {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        });
    }
}

/// [`PROFILE`] compiled once for the whole process, or `None` where libsandbox
/// refuses it.
fn bytecode() -> Option<&'static [u8]> {
    static BYTECODE: OnceLock<Option<Box<[u8]>>> = OnceLock::new();
    BYTECODE.get_or_init(compile).as_deref()
}

fn compile() -> Option<Box<[u8]>> {
    let mut error = ptr::null_mut();
    #[allow(unsafe_code)]
    unsafe {
        let profile = sandbox_compile_string(PROFILE.as_ptr(), ptr::null_mut(), &raw mut error);
        if profile.is_null() {
            sandbox_free_error(error);
            return None;
        }
        let compiled = &*profile;
        let bytecode = (!compiled.data.is_null())
            .then(|| Box::from(std::slice::from_raw_parts(compiled.data, compiled.size)));
        sandbox_free_profile(profile);
        bytecode
    }
}
