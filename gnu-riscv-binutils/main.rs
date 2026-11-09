use lazy_static::lazy_static;
use std::env;
use std::path::Path;
use std::fs::File;
use std::io::Read;
use std::path::PathBuf;
use std::process::Command;
#[cfg(windows)]
use std::iter::once;

const XESPV_VERSIONS: [&str; 3] = ["xespv3p0", "xespv2p2", "xespv2p1"];
// Each as/objdump binary has one opcode table baked in. DSP 2.2
// (0x6b / custom-0) lives in xespv3p0, which is also the default.
// Arch strings that name an extension here and no XESPV_VERSIONS token
// select the paired tool. First match wins; put more specific extensions first.
// DSP 2.1 (custom-3 / custom-2) -> xespv2p2.
const EXT_TO_TOOL_SUFFIX: &[(&str, &str)] = &[("xespdsp2p1", "xespv2p2")];
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

/// Tool suffix for the first EXT_TO_TOOL_SUFFIX key contained in `text`.
fn tool_suffix_for_ext(text: &str) -> Option<&'static str> {
    EXT_TO_TOOL_SUFFIX
        .iter()
        .find(|(ext, _)| text.contains(ext))
        .map(|(_, suffix)| *suffix)
}

/// Suffix from a Tag_RISCV_arch line. An xespv token wins, then a mapped
/// extension, then xesppie.
fn suffix_from_arch_line(line: &str) -> Option<&'static str> {
    if let Some(found_version) = XESPV_VERSIONS.iter().find(|v| line.contains(*v)) {
        esp_debug_trace!("arch line has {}", found_version);
        return Some(*found_version);
    }
    if let Some(suffix) = tool_suffix_for_ext(line) {
        esp_debug_trace!("found mapped extension, use {} suffix", suffix);
        return Some(suffix);
    }
    if line.contains("xesppie") {
        esp_debug_trace!("found xesppie extension, use xespv2p1 suffix");
        return Some("xespv2p1");
    }
    None
}

/// Suffix from a -march= value that names a mapped extension. Response files
/// are handled by the recursive parse_args_for_suffix call, so this only
/// looks at this argv. First matching argument wins.
fn march_ext_suffix(argv: &[String]) -> Option<&'static str> {
    argv.iter().find_map(|arg| {
        arg.strip_prefix(MARCH_ARG_PREFIX)
            .and_then(tool_suffix_for_ext)
    })
}

/// Parses command line arguments to determine tool suffix from -march= options
///
/// # Arguments
/// * `argv` - Command line arguments vector (without program name)
///
/// # Returns
/// Some(suffix) if found in arguments, None otherwise
fn parse_args_for_suffix(argv: &[String]) -> Option<String> {
    let mut tool_suffix = String::new();

    esp_debug_trace!("parse_args_for_suffix: entered");
    /* 1. Get suffix from the -march option of as/ld if any of XESPV_VERSIONS is specified */
    for arg in argv {
        if let Some(value) = arg.strip_prefix(MARCH_ARG_PREFIX) {
            if let Some(current_version) = XESPV_VERSIONS.iter().find(|v| value.contains(*v)) {
                tool_suffix = current_version.to_string();
                esp_debug_trace!("tool_suffix=\"{}\" based on XESPV_VERSIONS: \"{}\"", tool_suffix, arg);
            }
        }
        if let Some(value) = arg.strip_prefix('@') {
            // filepath may be surrounded by quotes and double quotes, so strip them
            esp_debug_trace!("processing response file=\"{}\"", value);
            let value = value.trim_matches('"').trim_matches('\'');
            esp_debug_trace!("quotes stripped=\"{}\"", value);

            match std::fs::read_to_string(value) {
                Ok(content) => {
                    let flags: Vec<String> = content.split_whitespace().map(|s| s.to_string()).collect();

                    if let Some(suffix) = parse_args_for_suffix(&flags) {
                        tool_suffix = suffix;
                        esp_debug_trace!("tool_suffix=\"{}\" based on response file \"{}\"", tool_suffix, value);
                    }
                }
                Err(e) => {
                    esp_debug_trace!("Warning: Failed to read response file \"{}\": {}", value, e);
                    // Continue processing other arguments instead of panicking
                }
            }
        }
    }

    // Mapped extensions have no xespv token. Without this, the wrapper
    // defaults to xespv3p0 and assembles them with the wrong opcode map.
    // An explicit xespv version already stored in tool_suffix keeps priority.
    if tool_suffix.is_empty() {
        if let Some(suffix) = march_ext_suffix(argv) {
            tool_suffix = suffix.to_string();
            esp_debug_trace!("found mapped extension, use {} suffix", suffix);
        }
    }

    esp_debug_trace!("parse_args_for_suffix: return tool_suffix=\"{}\"", tool_suffix);
    if !tool_suffix.is_empty() {
        return Some(tool_suffix);
    }

    None
}

/// Determines the tool suffix to use based on command line arguments or ELF file analysis
///
/// Priority order:
/// 1. Get suffix from the -march option of as/ld if any of XESPV_VERSIONS is specified
/// 2. -march names a mapped extension and no xespv version: paired tool suffix
/// 3. Objdump: analyze ELF files to determine extension from Tag_RISCV_arch
/// 4. Fallback to default suffix (first item in XESPV_VERSIONS)
///
/// # Returns
/// String suffix to append to the tool name
fn get_tool_suffix() -> String {
    // Skip the program name
    let argv: Vec<String> = env::args().skip(1).collect();

    // Try to get suffix from command line arguments first
    if let Some(suffix) = parse_args_for_suffix(&argv) {
        return suffix;
    }

    /* 2. For objdump only: analyze ELF files to determine extension from Tag_RISCV_arch */
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
        /* check in Tag_RISCV_arch in ELF files */
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
                if let Some(suffix) = suffix_from_arch_line(line) {
                    esp_debug_trace!("file {} has {}", path.display(), suffix);
                    return suffix.to_string();
                }
            }
        }
    }

    /* 3. Use default suffix if not found */
    esp_debug_trace!("use default tool_suffix=\"{}\"", XESPV_VERSIONS[0]);

    String::from(XESPV_VERSIONS[0])
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::sync::atomic::{AtomicU64, Ordering};

    fn args(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| (*s).to_string()).collect()
    }

    fn temp_file(name: &str, contents: &[u8]) -> PathBuf {
        static N: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "riscv-binutil-wrapper-{}-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed),
            name
        ));
        fs::write(&path, contents).unwrap();
        path
    }

    #[test]
    fn ext_map_selects_paired_suffix() {
        assert_eq!(
            tool_suffix_for_ext("rv32imc_xespdsp2p1"),
            Some("xespv2p2")
        );
        assert_eq!(tool_suffix_for_ext("rv32imc"), None);
    }

    #[test]
    fn march_ext_ignores_args_without_the_extension() {
        assert_eq!(march_ext_suffix(&args(&["-o", "out.o"])), None);
        assert_eq!(
            march_ext_suffix(&args(&[
                "-march=rv32imc",
                "-march=rv32imc_xespdsp2p1_zicsr",
            ])),
            Some("xespv2p2")
        );
    }

    #[test]
    fn march_xespv_version_selects_that_tool() {
        assert_eq!(
            parse_args_for_suffix(&args(&["-march=rv32imc_xespv3p0"])).as_deref(),
            Some("xespv3p0")
        );
        assert_eq!(
            parse_args_for_suffix(&args(&["-march=rv32imc_xespv2p2"])).as_deref(),
            Some("xespv2p2")
        );
        assert_eq!(
            parse_args_for_suffix(&args(&["-march=rv32imc_xespv2p1"])).as_deref(),
            Some("xespv2p1")
        );
    }

    #[test]
    fn march_without_version_or_mapped_ext_returns_none() {
        assert_eq!(parse_args_for_suffix(&args(&["-march=rv32imc"])), None);
        assert_eq!(parse_args_for_suffix(&[]), None);
    }

    #[test]
    fn mapped_ext_selects_paired_tool_when_no_xespv_token() {
        assert_eq!(
            parse_args_for_suffix(&args(&["-march=rv32imc_xespdsp2p1"])).as_deref(),
            Some("xespv2p2")
        );
    }

    #[test]
    fn explicit_xespv_beats_mapped_extension() {
        assert_eq!(
            parse_args_for_suffix(&args(&[
                "-march=rv32imc_xespdsp2p1",
                "-march=rv32imc_xespv3p0",
            ]))
            .as_deref(),
            Some("xespv3p0")
        );
        assert_eq!(
            parse_args_for_suffix(&args(&["-march=rv32imc_xespdsp2p1_xespv2p1"])).as_deref(),
            Some("xespv2p1")
        );
    }

    #[test]
    fn later_xespv_march_replaces_earlier_one() {
        assert_eq!(
            parse_args_for_suffix(&args(&[
                "-march=rv32imc_xespv2p1",
                "-march=rv32imc_xespv3p0",
            ]))
            .as_deref(),
            Some("xespv3p0")
        );
    }

    #[test]
    fn xespv_table_order_wins_inside_one_march_value() {
        assert_eq!(
            parse_args_for_suffix(&args(&["-march=rv32imc_xespv2p2_xespv3p0"])).as_deref(),
            Some("xespv3p0")
        );
    }

    #[test]
    fn response_file_supplies_mapped_extension() {
        let path = temp_file("dsp.rsp", b"-march=rv32imc_xespdsp2p1\n");
        let flag = format!("@{}", path.display());
        assert_eq!(
            parse_args_for_suffix(&args(&[&flag])).as_deref(),
            Some("xespv2p2")
        );
        let quoted = format!("@\"{}\"", path.display());
        assert_eq!(
            parse_args_for_suffix(&args(&[&quoted])).as_deref(),
            Some("xespv2p2")
        );
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn response_file_xespv_replaces_cmdline_version() {
        let path = temp_file("ver.rsp", b"-march=rv32imc_xespv2p1\n");
        let flag = format!("@{}", path.display());
        assert_eq!(
            parse_args_for_suffix(&args(&["-march=rv32imc_xespv3p0", &flag])).as_deref(),
            Some("xespv2p1")
        );
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn missing_response_file_keeps_other_suffix() {
        assert_eq!(
            parse_args_for_suffix(&args(&[
                "-march=rv32imc_xespv2p2",
                "@no-such-response-file-riscv-wrapper",
            ]))
            .as_deref(),
            Some("xespv2p2")
        );
        assert_eq!(
            parse_args_for_suffix(&args(&["@no-such-response-file-riscv-wrapper"])),
            None
        );
    }

    #[test]
    fn arch_line_priority() {
        assert_eq!(
            suffix_from_arch_line("Tag_RISCV_arch: rv32imc_xespv3p0"),
            Some("xespv3p0")
        );
        assert_eq!(
            suffix_from_arch_line("Tag_RISCV_arch: rv32imc_xespdsp2p1"),
            Some("xespv2p2")
        );
        assert_eq!(
            suffix_from_arch_line("Tag_RISCV_arch: rv32imc_xespdsp2p1_xespv3p0"),
            Some("xespv3p0")
        );
        assert_eq!(
            suffix_from_arch_line("Tag_RISCV_arch: rv32imc_xesppie"),
            Some("xespv2p1")
        );
        assert_eq!(
            suffix_from_arch_line("Tag_RISCV_arch: rv32imc_xespdsp2p1_xesppie"),
            Some("xespv2p2")
        );
        assert_eq!(suffix_from_arch_line("Tag_RISCV_arch: rv32imc"), None);
    }

    #[test]
    fn file_magic_detects_elf_and_archive() {
        let elf = temp_file("a.elf", b"\x7fELF and then junk");
        let ar = temp_file("lib.a", b"!<arch>\nsymbol table");
        let other = temp_file("note.txt", b"not a binary");
        assert!(is_elf_file(&elf));
        assert!(!is_static_lib(&elf));
        assert!(is_elf_or_static_lib(&elf));
        assert!(is_static_lib(&ar));
        assert!(!is_elf_file(&ar));
        assert!(is_elf_or_static_lib(&ar));
        assert!(!is_elf_file(&other));
        assert!(!is_static_lib(&other));
        assert!(!is_elf_or_static_lib(&other));
        assert!(!is_elf_file(Path::new("missing-riscv-wrapper-file")));
        fs::remove_file(elf).unwrap();
        fs::remove_file(ar).unwrap();
        fs::remove_file(other).unwrap();
    }
}
