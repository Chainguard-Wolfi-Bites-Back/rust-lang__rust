//! `run-make-support` is a support library for run-make tests. It provides command wrappers and
//! convenience utility functions to help test writers reduce duplication. The support library
//! notably is built via cargo: this means that if your test wants some non-trivial utility, such
//! as `object` or `wasmparser`, they can be re-exported and be made available through this library.

mod command;
pub mod diff;
pub mod env_checked;
pub mod external_deps;
pub mod fs_wrapper;
mod macros;
pub mod run;
pub mod targets;

use std::fs;
use std::io;
use std::panic;
use std::path::{Path, PathBuf};

pub use bstr;
pub use gimli;
pub use object;
pub use regex;
pub use wasmparser;

// Re-exports of external dependencies.
pub use external_deps::{cc, clang, htmldocck, llvm, python, rustc, rustdoc};

pub use cc::{cc, extra_c_flags, extra_cxx_flags, Cc};
pub use clang::{clang, Clang};
pub use htmldocck::htmldocck;
pub use llvm::{
    llvm_ar, llvm_filecheck, llvm_objdump, llvm_profdata, llvm_readobj, LlvmAr, LlvmFilecheck,
    LlvmObjdump, LlvmProfdata, LlvmReadobj,
};
pub use python::python_command;
pub use rustc::{aux_build, bare_rustc, rustc, Rustc};
pub use rustdoc::{bare_rustdoc, rustdoc, Rustdoc};

pub use diff::{diff, Diff};

pub use env_checked::{env_var, env_var_os};

pub use run::{cmd, run, run_fail, run_with_args};

pub use targets::{is_darwin, is_msvc, is_windows, target, uname};

use command::{Command, CompletedProcess};

/// `AR`
#[track_caller]
pub fn ar(inputs: &[impl AsRef<Path>], output_path: impl AsRef<Path>) {
    let output = fs::File::create(&output_path).expect(&format!(
        "the file in path \"{}\" could not be created",
        output_path.as_ref().display()
    ));
    let mut builder = ar::Builder::new(output);
    for input in inputs {
        builder.append_path(input).unwrap();
    }
}

/// Returns the path for a local test file.
pub fn path<P: AsRef<Path>>(p: P) -> PathBuf {
    cwd().join(p.as_ref())
}

/// Path to the root rust-lang/rust source checkout.
#[must_use]
pub fn source_root() -> PathBuf {
    env_var("SOURCE_ROOT").into()
}

/// Creates a new symlink to a path on the filesystem, adjusting for Windows or Unix.
#[cfg(target_family = "windows")]
pub fn create_symlink<P: AsRef<Path>, Q: AsRef<Path>>(original: P, link: Q) {
    if link.as_ref().exists() {
        std::fs::remove_dir(link.as_ref()).unwrap();
    }
    use std::os::windows::fs;
    fs::symlink_file(original.as_ref(), link.as_ref()).expect(&format!(
        "failed to create symlink {:?} for {:?}",
        link.as_ref().display(),
        original.as_ref().display(),
    ));
}

/// Creates a new symlink to a path on the filesystem, adjusting for Windows or Unix.
#[cfg(target_family = "unix")]
pub fn create_symlink<P: AsRef<Path>, Q: AsRef<Path>>(original: P, link: Q) {
    if link.as_ref().exists() {
        std::fs::remove_dir(link.as_ref()).unwrap();
    }
    use std::os::unix::fs;
    fs::symlink(original.as_ref(), link.as_ref()).expect(&format!(
        "failed to create symlink {:?} for {:?}",
        link.as_ref().display(),
        original.as_ref().display(),
    ));
}

/// Construct the static library name based on the platform.
#[must_use]
pub fn static_lib_name(name: &str) -> String {
    // See tools.mk (irrelevant lines omitted):
    //
    // ```makefile
    // ifeq ($(UNAME),Darwin)
    //     STATICLIB = $(TMPDIR)/lib$(1).a
    // else
    //     ifdef IS_WINDOWS
    //         ifdef IS_MSVC
    //             STATICLIB = $(TMPDIR)/$(1).lib
    //         else
    //             STATICLIB = $(TMPDIR)/lib$(1).a
    //         endif
    //     else
    //         STATICLIB = $(TMPDIR)/lib$(1).a
    //     endif
    // endif
    // ```
    assert!(!name.contains(char::is_whitespace), "static library name cannot contain whitespace");

    if is_msvc() { format!("{name}.lib") } else { format!("lib{name}.a") }
}

/// Construct the dynamic library name based on the platform.
#[must_use]
pub fn dynamic_lib_name(name: &str) -> String {
    // See tools.mk (irrelevant lines omitted):
    //
    // ```makefile
    // ifeq ($(UNAME),Darwin)
    //     DYLIB = $(TMPDIR)/lib$(1).dylib
    // else
    //     ifdef IS_WINDOWS
    //         DYLIB = $(TMPDIR)/$(1).dll
    //     else
    //         DYLIB = $(TMPDIR)/lib$(1).so
    //     endif
    // endif
    // ```
    assert!(!name.contains(char::is_whitespace), "dynamic library name cannot contain whitespace");

    let extension = dynamic_lib_extension();
    if is_darwin() {
        format!("lib{name}.{extension}")
    } else if is_windows() {
        format!("{name}.{extension}")
    } else {
        format!("lib{name}.{extension}")
    }
}

#[must_use]
pub fn dynamic_lib_extension() -> &'static str {
    if is_darwin() {
        "dylib"
    } else if is_windows() {
        "dll"
    } else {
        "so"
    }
}

/// Generate the name a rust library (rlib) would have.
#[must_use]
pub fn rust_lib_name(name: &str) -> String {
    format!("lib{name}.rlib")
}

/// Construct the binary name based on platform.
#[must_use]
pub fn bin_name(name: &str) -> String {
    if is_windows() { format!("{name}.exe") } else { name.to_string() }
}

/// Return the current working directory.
#[must_use]
pub fn cwd() -> PathBuf {
    std::env::current_dir().unwrap()
}

// FIXME(Oneirical): This will no longer be required after compiletest receives the ability
// to manipulate read-only files. See https://github.com/rust-lang/rust/issues/126334
/// Ensure that the path P is read-only while the test runs, and restore original permissions
/// at the end so compiletest can clean up.
/// This will panic on Windows if the path is a directory (as it would otherwise do nothing)
#[track_caller]
pub fn test_while_readonly<P: AsRef<Path>, F: FnOnce() + std::panic::UnwindSafe>(
    path: P,
    closure: F,
) {
    let path = path.as_ref();
    if is_windows() && path.is_dir() {
        eprintln!("This helper function cannot be used on Windows to make directories readonly.");
        eprintln!(
            "See the official documentation:
            https://doc.rust-lang.org/std/fs/struct.Permissions.html#method.set_readonly"
        );
        panic!("`test_while_readonly` on directory detected while on Windows.");
    }
    let metadata = fs_wrapper::metadata(&path);
    let original_perms = metadata.permissions();

    let mut new_perms = original_perms.clone();
    new_perms.set_readonly(true);
    fs_wrapper::set_permissions(&path, new_perms);

    let success = std::panic::catch_unwind(closure);

    fs_wrapper::set_permissions(&path, original_perms);
    success.unwrap();
}

/// Browse the directory `path` non-recursively and return all files which respect the parameters
/// outlined by `closure`.
#[track_caller]
pub fn shallow_find_files<P: AsRef<Path>, F: Fn(&PathBuf) -> bool>(
    path: P,
    filter: F,
) -> Vec<PathBuf> {
    let mut matching_files = Vec::new();
    for entry in fs_wrapper::read_dir(path) {
        let entry = entry.expect("failed to read directory entry.");
        let path = entry.path();

        if path.is_file() && filter(&path) {
            matching_files.push(path);
        }
    }
    matching_files
}

/// Returns true if the filename at `path` starts with `prefix`.
pub fn has_prefix<P: AsRef<Path>>(path: P, prefix: &str) -> bool {
    path.as_ref().file_name().is_some_and(|name| name.to_str().unwrap().starts_with(prefix))
}

/// Returns true if the filename at `path` has the extension `extension`.
pub fn has_extension<P: AsRef<Path>>(path: P, extension: &str) -> bool {
    path.as_ref().extension().is_some_and(|ext| ext == extension)
}

/// Returns true if the filename at `path` does not contain `expected`.
pub fn not_contains<P: AsRef<Path>>(path: P, expected: &str) -> bool {
    !path.as_ref().file_name().is_some_and(|name| name.to_str().unwrap().contains(expected))
}

/// Builds a static lib (`.lib` on Windows MSVC and `.a` for the rest) with the given name.
#[track_caller]
pub fn build_native_static_lib(lib_name: &str) -> PathBuf {
    let obj_file = if is_msvc() { format!("{lib_name}") } else { format!("{lib_name}.o") };
    let src = format!("{lib_name}.c");
    let lib_path = static_lib_name(lib_name);
    if is_msvc() {
        cc().arg("-c").out_exe(&obj_file).input(src).run();
    } else {
        cc().arg("-v").arg("-c").out_exe(&obj_file).input(src).run();
    };
    let obj_file = if is_msvc() {
        PathBuf::from(format!("{lib_name}.obj"))
    } else {
        PathBuf::from(format!("{lib_name}.o"))
    };
    llvm_ar().obj_to_ar().output_input(&lib_path, &obj_file).run();
    path(lib_path)
}

/// Returns true if the filename at `path` is not in `expected`.
pub fn filename_not_in_denylist<P: AsRef<Path>, V: AsRef<[String]>>(path: P, expected: V) -> bool {
    let expected = expected.as_ref();
    path.as_ref()
        .file_name()
        .is_some_and(|name| !expected.contains(&name.to_str().unwrap().to_owned()))
}

/// Returns true if the filename at `path` ends with `suffix`.
pub fn has_suffix<P: AsRef<Path>>(path: P, suffix: &str) -> bool {
    path.as_ref().file_name().is_some_and(|name| name.to_str().unwrap().ends_with(suffix))
}

/// Gathers all files in the current working directory that have the extension `ext`, and counts
/// the number of lines within that contain a match with the regex pattern `re`.
pub fn count_regex_matches_in_files_with_extension(re: &regex::Regex, ext: &str) -> usize {
    let fetched_files = shallow_find_files(cwd(), |path| has_extension(path, ext));

    let mut count = 0;
    for file in fetched_files {
        let content = fs_wrapper::read_to_string(file);
        count += content.lines().filter(|line| re.is_match(&line)).count();
    }

    count
}

/// Use `cygpath -w` on a path to get a Windows path string back. This assumes that `cygpath` is
/// available on the platform!
#[track_caller]
#[must_use]
pub fn cygpath_windows<P: AsRef<Path>>(path: P) -> String {
    let caller = panic::Location::caller();
    let mut cygpath = Command::new("cygpath");
    cygpath.arg("-w");
    cygpath.arg(path.as_ref());
    let output = cygpath.run();
    if !output.status().success() {
        handle_failed_output(&cygpath, output, caller.line());
    }
    // cygpath -w can attach a newline
    output.stdout_utf8().trim().to_string()
}

pub(crate) fn handle_failed_output(
    cmd: &Command,
    output: CompletedProcess,
    caller_line_number: u32,
) -> ! {
    if output.status().success() {
        eprintln!("command unexpectedly succeeded at line {caller_line_number}");
    } else {
        eprintln!("command failed at line {caller_line_number}");
    }
    eprintln!("{cmd:?}");
    eprintln!("output status: `{}`", output.status());
    eprintln!("=== STDOUT ===\n{}\n\n", output.stdout_utf8());
    eprintln!("=== STDERR ===\n{}\n\n", output.stderr_utf8());
    std::process::exit(1)
}

/// Set the runtime library path as needed for running the host rustc/rustdoc/etc.
pub fn set_host_rpath(cmd: &mut Command) {
    let ld_lib_path_envvar = env_var("LD_LIB_PATH_ENVVAR");
    cmd.env(&ld_lib_path_envvar, {
        let mut paths = vec![];
        paths.push(cwd());
        paths.push(PathBuf::from(env_var("HOST_RPATH_DIR")));
        for p in std::env::split_paths(&env_var(&ld_lib_path_envvar)) {
            paths.push(p.to_path_buf());
        }
        std::env::join_paths(paths.iter()).unwrap()
    });
}

/// Read the contents of a file that cannot simply be read by
/// read_to_string, due to invalid utf8 data, then assert that it contains `expected`.
#[track_caller]
pub fn invalid_utf8_contains<P: AsRef<Path>, S: AsRef<str>>(path: P, expected: S) {
    let buffer = fs_wrapper::read(path.as_ref());
    let expected = expected.as_ref();
    if !String::from_utf8_lossy(&buffer).contains(expected) {
        eprintln!("=== FILE CONTENTS (LOSSY) ===");
        eprintln!("{}", String::from_utf8_lossy(&buffer));
        eprintln!("=== SPECIFIED TEXT ===");
        eprintln!("{}", expected);
        panic!("specified text was not found in file");
    }
}

/// Read the contents of a file that cannot simply be read by
/// read_to_string, due to invalid utf8 data, then assert that it does not contain `expected`.
#[track_caller]
pub fn invalid_utf8_not_contains<P: AsRef<Path>, S: AsRef<str>>(path: P, expected: S) {
    let buffer = fs_wrapper::read(path.as_ref());
    let expected = expected.as_ref();
    if String::from_utf8_lossy(&buffer).contains(expected) {
        eprintln!("=== FILE CONTENTS (LOSSY) ===");
        eprintln!("{}", String::from_utf8_lossy(&buffer));
        eprintln!("=== SPECIFIED TEXT ===");
        eprintln!("{}", expected);
        panic!("specified text was unexpectedly found in file");
    }
}

/// Copy a directory into another.
pub fn copy_dir_all(src: impl AsRef<Path>, dst: impl AsRef<Path>) {
    fn copy_dir_all_inner(src: impl AsRef<Path>, dst: impl AsRef<Path>) -> io::Result<()> {
        let dst = dst.as_ref();
        if !dst.is_dir() {
            std::fs::create_dir_all(&dst)?;
        }
        for entry in std::fs::read_dir(src)? {
            let entry = entry?;
            let ty = entry.file_type()?;
            if ty.is_dir() {
                copy_dir_all_inner(entry.path(), dst.join(entry.file_name()))?;
            } else {
                std::fs::copy(entry.path(), dst.join(entry.file_name()))?;
            }
        }
        Ok(())
    }

    if let Err(e) = copy_dir_all_inner(&src, &dst) {
        // Trying to give more context about what exactly caused the failure
        panic!(
            "failed to copy `{}` to `{}`: {:?}",
            src.as_ref().display(),
            dst.as_ref().display(),
            e
        );
    }
}

/// Check that all files in `dir1` exist and have the same content in `dir2`. Panic otherwise.
pub fn recursive_diff(dir1: impl AsRef<Path>, dir2: impl AsRef<Path>) {
    let dir2 = dir2.as_ref();
    read_dir(dir1, |entry_path| {
        let entry_name = entry_path.file_name().unwrap();
        if entry_path.is_dir() {
            recursive_diff(&entry_path, &dir2.join(entry_name));
        } else {
            let path2 = dir2.join(entry_name);
            let file1 = fs_wrapper::read(&entry_path);
            let file2 = fs_wrapper::read(&path2);

            // We don't use `assert_eq!` because they are `Vec<u8>`, so not great for display.
            // Why not using String? Because there might be minified files or even potentially
            // binary ones, so that would display useless output.
            assert!(
                file1 == file2,
                "`{}` and `{}` have different content",
                entry_path.display(),
                path2.display(),
            );
        }
    });
}

pub fn read_dir<F: FnMut(&Path)>(dir: impl AsRef<Path>, mut callback: F) {
    for entry in fs_wrapper::read_dir(dir) {
        callback(&entry.unwrap().path());
    }
}

/// Check that `actual` is equal to `expected`. Panic otherwise.
#[track_caller]
pub fn assert_equals<S1: AsRef<str>, S2: AsRef<str>>(actual: S1, expected: S2) {
    let actual = actual.as_ref();
    let expected = expected.as_ref();
    if actual != expected {
        eprintln!("=== ACTUAL TEXT ===");
        eprintln!("{}", actual);
        eprintln!("=== EXPECTED ===");
        eprintln!("{}", expected);
        panic!("expected text was not found in actual text");
    }
}

/// Check that `haystack` contains `needle`. Panic otherwise.
#[track_caller]
pub fn assert_contains<S1: AsRef<str>, S2: AsRef<str>>(haystack: S1, needle: S2) {
    let haystack = haystack.as_ref();
    let needle = needle.as_ref();
    if !haystack.contains(needle) {
        eprintln!("=== HAYSTACK ===");
        eprintln!("{}", haystack);
        eprintln!("=== NEEDLE ===");
        eprintln!("{}", needle);
        panic!("needle was not found in haystack");
    }
}

/// Check that `haystack` does not contain `needle`. Panic otherwise.
#[track_caller]
pub fn assert_not_contains<S1: AsRef<str>, S2: AsRef<str>>(haystack: S1, needle: S2) {
    let haystack = haystack.as_ref();
    let needle = needle.as_ref();
    if haystack.contains(needle) {
        eprintln!("=== HAYSTACK ===");
        eprintln!("{}", haystack);
        eprintln!("=== NEEDLE ===");
        eprintln!("{}", needle);
        panic!("needle was unexpectedly found in haystack");
    }
}

/// This function is designed for running commands in a temporary directory
/// that is cleared after the function ends.
///
/// What this function does:
/// 1) Creates a temporary directory (`tmpdir`)
/// 2) Copies all files from the current directory to `tmpdir`
/// 3) Changes the current working directory to `tmpdir`
/// 4) Calls `callback`
/// 5) Switches working directory back to the original one
/// 6) Removes `tmpdir`
pub fn run_in_tmpdir<F: FnOnce()>(callback: F) {
    let original_dir = cwd();
    let tmpdir = original_dir.join("../temporary-directory");
    copy_dir_all(".", &tmpdir);

    std::env::set_current_dir(&tmpdir).unwrap();
    callback();
    std::env::set_current_dir(original_dir).unwrap();
    fs::remove_dir_all(tmpdir).unwrap();
}
