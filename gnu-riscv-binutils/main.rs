use lazy_static::lazy_static;
use std::env;
use std::path::Path;
use std::fs::File;
use std::io::Read;
use std::path::PathBuf;
use std::process::Command;
#[cfg(windows)]
use std::iter::once;

const XESPV_VERSIONS: [&str; 2] = ["xespv2p2", "xespv2p1"];
const XESPV_ARG_PREFIX: &str = "-mespv-spec=";
const MARCH_ARG_PREFIX: &str = "-march=";
const ELF_MAGIC: [u8; 4] = [0x7F, b'E', b'L', b'F'];
const AR_MAGIC: &[u8] = b"!<arch>\n";
#[cfg(windows)]
const MAX_PATH_SIZE: usize = 260;

lazy_static! {
    static ref ESP_DEBUG_TRACE: bool = env::var("ESP_DEBUG_TRACE").is_ok();
}

macro_rules! esp_debug_trace {
    ($($arg:tt)*) => {
        {
            if *ESP_DEBUG_TRACE {
                println!($($arg)*);
            }
        }
    };
}

#[cfg(windows)]
extern "system" {
    fn GetLongPathNameW(lpszShortPath: *const u16, lpszLongPath: *mut u16, cchBuffer: u32) -> u32;
    fn GetShortPathNameW(lpszLongPath: *const u16, lpszShortPath: *mut u16, cchBuffer: u32) -> u32;
    fn GetLastError() -> u32;
}

#[cfg(windows)]
/// Generic Windows API wrapper for path name conversion functions
///
/// # Arguments
/// * `path` - The path to convert
/// * `api_function` - Windows API function to call (GetLongPathNameW or GetShortPathNameW)
///
/// # Returns
/// Converted PathBuf
fn get_path_name(
    path: &PathBuf,
    api_function: unsafe extern "system" fn(*const u16, *mut u16, u32) -> u32,
) -> PathBuf {
    use std::ffi::OsString;
    use std::os::windows::ffi::OsStrExt;
    use std::os::windows::ffi::OsStringExt;

    // Convert the Rust string to a UTF-16 wide string
    let wide_path: Vec<u16> = path.as_os_str().encode_wide().chain(once(0)).collect();

    // Start with an initial buffer size (default is MAX_PATH_SIZE(260))
    let mut buffer_size = MAX_PATH_SIZE;
    let mut buffer: Vec<u16> = vec![0; buffer_size];

    loop {
        // Call the provided API function
        let length =
            unsafe { api_function(wide_path.as_ptr(), buffer.as_mut_ptr(), buffer.len() as u32) };

        assert_ne!(
            length,
            0,
            "Failed to get path name. Error code: {}",
            unsafe { GetLastError() }
        );
        if length > buffer_size as u32 {
            // The buffer is too small. The API function returns required size of buffer.
            // Resize the buffer with that value.
            buffer_size = length as usize;
            buffer.resize(buffer_size, 0);
        } else {
            // The function succeeded, convert the buffer to a Rust String
            return PathBuf::from(OsString::from_wide(&buffer[..length as usize]));
        }
    }
}

#[cfg(windows)]
/// Converts a short path name to a long path name on Windows
fn get_long_path_name(short_path: &PathBuf) -> PathBuf {
    get_path_name(short_path, GetLongPathNameW)
}

#[cfg(windows)]
/// Converts a long path name to a short path name on Windows
fn get_short_path_name(long_path: &PathBuf) -> PathBuf {
    get_path_name(long_path, GetShortPathNameW)
}

/// Checks if a file starts with the specified magic bytes
fn check_file_magic(path: &Path, magic: &[u8]) -> bool {
    File::open(path)
        .and_then(|mut f| {
            let mut buffer = vec![0u8; magic.len()];
            f.read_exact(&mut buffer).map(|_| buffer)
        })
        .map(|buffer| buffer == magic)
        .unwrap_or(false)
}

/// Checks if a file is an ELF file by examining its magic bytes
fn is_elf_file(path: &Path) -> bool {
    check_file_magic(path, &ELF_MAGIC)
}

/// Checks if a file is a static library (ar archive) by examining its magic bytes
fn is_static_lib(path: &Path) -> bool {
    check_file_magic(path, AR_MAGIC)
}

/// Checks if a file is either an ELF file or a static library
fn is_elf_or_static_lib(path: &Path) -> bool {
    path.is_file() && (is_elf_file(path) || is_static_lib(path))
}

/// Determines the tool suffix to use based on command line arguments or ELF file analysis
///
/// Priority order:
/// 1. Use suffix from "-mespv-spec=" command line argument if provided
/// 2. Get suffix from the -march option of as/ld if any of XESPV_VERSIONS is specified
/// 3. Objdump: analyze ELF files to determine extension from Tag_RISCV_arch
/// 4. Fallback to default suffix (first item in XESPV_VERSIONS)
///
/// # Returns
/// String suffix to append to the tool name
fn get_tool_suffix() -> String {
    let mut tool_suffix = String::new();
    let mut march_extension = String::new();

    // Skip the program name
    let argv: Vec<String> = env::args().skip(1).collect();

    /* 1. Iterate and check all "-mespv-spec=" arguments
     * The last one will be applied.
     */
    /* 2. Get suffix from the -march option of as/ld if any of XESPV_VERSIONS is specified */
    for arg in &argv {
        if let Some(value) = arg.strip_prefix(XESPV_ARG_PREFIX) {
            tool_suffix = format!("xespv{}", value);
            esp_debug_trace!("tool_suffix=\"{}\"", tool_suffix);
        }
        if let Some(value) = arg.strip_prefix(MARCH_ARG_PREFIX) {
            if let Some(current_version) = XESPV_VERSIONS.iter().find(|v| value.contains(*v)) {
                march_extension = current_version.to_string();
                esp_debug_trace!("march_extension=\"{}\"", march_extension);
            }
        }
    }

    if !tool_suffix.is_empty() {
        esp_debug_trace!("return \"{}\" based on {}", tool_suffix, XESPV_ARG_PREFIX);
        return tool_suffix;
    }

    if !march_extension.is_empty() {
        esp_debug_trace!("return \"{}\" based on {}", march_extension, MARCH_ARG_PREFIX);
        return march_extension;
    }

    /* 3. For objdump only: analyze ELF files to determine extension from Tag_RISCV_arch */
    let (wrapper_path, is_short_path) = get_current_exe_path();
    let stem = wrapper_path.file_stem().expect("file stem").to_string_lossy();
    esp_debug_trace!("stem=\"{}\"", stem);
    if stem.contains("objdump") {
        let ext  = wrapper_path.extension().map(|e| e.to_string_lossy());
        let readelf_filename = {
            let base = match stem.rfind('-') {
                Some(pos) => &stem[..pos + 1],
                None => "",
            };

            if let Some(ext) = ext {
                format!("{}readelf.{}", base, ext)
            } else {
                format!("{}readelf", base)
            }
        };

        let mut readelf_exe_path = wrapper_path.clone();
        readelf_exe_path.set_file_name(readelf_filename);
        readelf_exe_path = correct_path(readelf_exe_path, is_short_path);

        esp_debug_trace!("readelf_exe_path=\"{}\"", readelf_exe_path.display());
        /* 2. check in Tag_RISCV_arch in ELF files */
        for arg in &argv {
            let path = Path::new(arg);
            if !is_elf_or_static_lib(path) {
                continue
            }
            let output = Command::new(&readelf_exe_path)
                .arg("-A")
                .arg(path)
                .output()
                .unwrap_or_else(|err| panic!("Failed to run readelf on {}: {}", path.display(), err));
            let stdout = String::from_utf8_lossy(&output.stdout);
            if let Some(line) = stdout.lines().find(|l| l.contains("Tag_RISCV_arch")) {
                if let Some(found_version) = XESPV_VERSIONS.iter().find(|v| line.contains(*v)) {
                    esp_debug_trace!("file {} has {}", path.display(), found_version);
                    return found_version.to_string();
                }
                if line.contains("xesppie") {
                    esp_debug_trace!("found xesppie extension");
                    return "xesppie".to_string();
                }
            }
        }
    }

    /* 4. Use default suffix if not found */
    if tool_suffix.is_empty() {
        tool_suffix = String::from(XESPV_VERSIONS[0]);
        esp_debug_trace!("use default tool_suffix=\"{}\"", tool_suffix);
    }

    tool_suffix
}

/// Gets the current executable path and determines if it's using short path names on Windows
///
/// # Returns
/// Tuple of (executable_path, is_short_path)
fn get_current_exe_path() -> (PathBuf, bool) {
    #[cfg(windows)]
    {
        let exe_path = env::current_exe().expect("failed to get executable path");
        let short_path = get_short_path_name(&exe_path);
        let is_short_path = exe_path == short_path;

        let wrapper_path = if is_short_path {
            get_long_path_name(&exe_path)
        } else {
            exe_path
        };

        return (wrapper_path, is_short_path)
    }
    #[cfg(not(windows))]
    return (env::current_exe().expect("failed to get executable path"), false);
}

/// Corrects a path to use short path names on Windows if needed
///
/// # Arguments
/// * `path` - The path to correct
/// * `_is_short_path` - Whether to convert to short path (Windows only)
///
/// # Returns
/// Corrected PathBuf
fn correct_path(path: PathBuf, _is_short_path: bool) -> PathBuf {
    #[cfg(windows)]
    {
        if _is_short_path {
            get_short_path_name(&path)
        } else {
            path
        }
    }
    #[cfg(not(windows))]
    path
}


fn main() {
    let mut argv: Vec<String> = std::env::args().collect();
    let (wrapper_path, is_short_path) = get_current_exe_path();
    let tool_suffix = get_tool_suffix();

    // Remove all "-mespv-spec=" arguments from argv
    argv.retain(|arg| !arg.starts_with(XESPV_ARG_PREFIX));

    let stem = wrapper_path.file_stem().expect("file stem").to_string_lossy();
    let ext  = wrapper_path.extension().map(|e| e.to_string_lossy());

    // Insert suffix before extension
    let new_name = if let Some(ext) = ext {
        format!("{}-{}.{}", stem, tool_suffix, ext)
    } else {
        format!("{}-{}", stem, tool_suffix)
    };

    let mut new_exe_path = wrapper_path.clone();
    new_exe_path.set_file_name(new_name);

    argv[0] = correct_path(new_exe_path, is_short_path).display().to_string();

    esp_debug_trace!("Execute: {:?}", argv);
    exec(argv);
}

#[cfg(unix)]
/// Executes a command on Unix systems by replacing the current process
///
/// # Arguments
/// * `argv` - Command and arguments vector
fn exec(argv: Vec<String>) {
    use std::os::unix::process::CommandExt;
    let app = &argv[0];
    let args = &argv[1..];
    let err = Command::new(app)
        .args(args)
        .exec(); // exec replaces the current process on Unix

    eprintln!("{} {:?} failed with error({})", app, args, err);
    unreachable!();
}

#[cfg(windows)]
/// Executes a command on Windows by spawning a child process and exiting with its code
///
/// # Arguments
/// * `argv` - Command and arguments vector
fn exec(argv: Vec<String>) {
    use std::process::{exit, ExitStatus};

    let mut child = Command::new(argv.get(0).expect("app in argv[0]"))
        .args(&argv[1..])
        .spawn()
        .expect("Failed to start child process");

    let status: ExitStatus = child.wait().expect("Failed to wait for child process");

    esp_debug_trace!("Child process exited with code {:?}", status.code());
    match status.code() {
        Some(c) => exit(c),
        None => exit(-1),
    };
}

#[cfg(all(windows, target_pointer_width = "32"))]
#[no_mangle]
pub extern "C" fn _Unwind_Resume() {
    /*
     * Mingw for 32-bit windows usually does not have DWARF unwinder, and as a result,
     * the _Unwind_Resume function is absent in libraries. To avoid linking against
     * this missing function, the panic=abort option is specified for the win32
     * target in config.toml.
     *
     * However, Rust attempts to link with _Unwind_Resume() even with panic=abort
     * See https://github.com/rust-lang/rust/issues/79609
     *
     * panic=abort will never call _Unwind_Resume.
     * So, this dummy function is created just to make linker happy
     */
}
