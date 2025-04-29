use lazy_static::lazy_static;
use std::env;
use std::iter::once;
use std::path::Path;
#[cfg(windows)]
use std::path::PathBuf;

const CONFIG_ENV_NAME: &str = "XTENSA_GNU_CONFIG";
const XTENSA_TOOLCHAIN_PREFIX: &str = "xtensa-esp-elf-";
const XTENSA_TOOL_PARSE_ERROR: &str = "Called tool must have pattern \"xtensa-esp*-elf-*\"";

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
fn get_path_name(
    path: &PathBuf,
    api_function: unsafe extern "system" fn(*const u16, *mut u16, u32) -> u32,
) -> PathBuf {
    use std::ffi::OsString;
    use std::os::windows::ffi::OsStrExt;
    use std::os::windows::ffi::OsStringExt;

    // Convert the Rust string to a UTF-16 wide string
    let wide_path: Vec<u16> = path.as_os_str().encode_wide().chain(once(0)).collect();

    // Start with an initial buffer size (default is 260 for MAX_PATH)
    let mut buffer_size = 260;
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
            // The buffer is too small. The API function returns requiered size of buffer.
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
fn get_long_path_name(short_path: &PathBuf) -> PathBuf {
    get_path_name(short_path, GetLongPathNameW)
}

#[cfg(windows)]
fn get_short_path_name(long_path: &PathBuf) -> PathBuf {
    get_path_name(long_path, GetShortPathNameW)
}

fn main() {
    let wrapper_path;
    #[cfg(windows)]
    let short_path_using;
    let exe_path = env::current_exe().expect("Get executable path");
    #[cfg(windows)]
    {
        let short_path = get_short_path_name(&exe_path);
        short_path_using = exe_path == short_path;
        wrapper_path = if short_path_using {
            get_long_path_name(&exe_path)
        } else {
            exe_path
        }
    }
    #[cfg(unix)]
    {
        wrapper_path = exe_path;
    }
    let wrapper_name = Path::new(&wrapper_path)
        .file_name()
        .expect("Current exe has path")
        .to_str()
        .unwrap();

    let mut chip = "";
    let mut tool_name = Vec::<&str>::new();
    for (i, s) in wrapper_name.split('-').enumerate() {
        match i {
            0 => assert_eq!(s, "xtensa", "{}", XTENSA_TOOL_PARSE_ERROR),
            1 => chip = s,
            2 => assert_eq!(s, "elf", "{}", XTENSA_TOOL_PARSE_ERROR),
            _ => tool_name.push(s),
        }
    }
    let chip = chip;
    assert_ne!(chip, "esp", "Target chip can not be \"esp\"");
    assert_ne!(chip, "", "{}", XTENSA_TOOL_PARSE_ERROR);

    let tool_name = tool_name.join("-");
    assert_ne!(tool_name, "", "{}", XTENSA_TOOL_PARSE_ERROR);

    let bin_dir = wrapper_path
        .parent()
        .expect("Executable must be in some directory");

    /* Get tool path */
    let exec_path = bin_dir.join(format!("{}{}", XTENSA_TOOLCHAIN_PREFIX, tool_name));
    assert!(
        exec_path.try_exists().unwrap(),
        "Tool {} is not exist",
        exec_path.display()
    );

    let dynconfig_filename = format!("xtensa_{}.so", chip);
    /* Get dynconfig path */
    let dynconfig_path = bin_dir
        .parent()
        .expect("Toolchain must be in some directory")
        .join("lib")
        .join(dynconfig_filename.clone());

    assert!(
        dynconfig_path.try_exists().unwrap(),
        "Dynconfig for target {} is not exist ({})",
        chip,
        dynconfig_path.display()
    );

    #[cfg(windows)]
    let dynconfig_path = if short_path_using {
        get_short_path_name(&dynconfig_path)
    } else {
        dynconfig_path
    };

    /* Set XTENSA_GNU_CONFIG env variable */
    esp_debug_trace!("export {}={}", CONFIG_ENV_NAME, dynconfig_path.display());
    env::set_var(CONFIG_ENV_NAME, dynconfig_path);

    let mut argv: Vec<String> = std::env::args().peekable().collect();
    #[cfg(windows)]
    {
        argv[0] = if short_path_using {
            get_short_path_name(&exec_path).display().to_string()
        } else {
            exec_path.display().to_string()
        };
    }

    #[cfg(not(windows))]
    {
        argv[0] = exec_path.display().to_string();
    }

    if is_compiler(tool_name) {
        /* Need to add mdynconfig option for using the right multilib instance */
        let dynconfig_option = format!("-mdynconfig={}", dynconfig_filename);
        argv.insert(1, dynconfig_option);
    }

    esp_debug_trace!("Execute: {:?}", argv);
    exec(argv);
}

#[cfg(unix)]
fn exec(argv: Vec<String>) {
    use std::ffi::CString;
    use std::ptr::null;

    let argv: Vec<CString> = argv
        .iter()
        .map(|x| CString::new(x.as_bytes()).unwrap())
        .collect();

    let argv: Vec<_> = argv
        .iter()
        .map(|x| x.as_ptr())
        .chain(once(null()))
        .collect();

    let app = *argv.first().expect("app in argv[0]");

    unsafe { libc::execv(app, argv.as_ptr()) };
    println!(
        "execv errno ({})",
        std::io::Error::last_os_error().raw_os_error().unwrap()
    );
    unreachable!();
}

#[cfg(windows)]
fn exec(argv: Vec<String>) {
    use std::process::{exit, Command, ExitStatus};

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

fn is_compiler(tool_name: String) -> bool {
    /* consider tools:
     * xtensa-esp-elf-cc[.exe]
     * xtensa-esp-elf-gcc[.exe]
     * xtensa-esp-elf-g++[.exe]
     * xtensa-esp-elf-c++[.exe]
     * xtensa-esp-elf-gcc-13.1.0[.exe]
     */
    #[cfg(windows)]
    let tool_name = match tool_name.strip_suffix(".exe") {
        Some(s) => s.to_owned(),
        None => tool_name,
    };

    if ["cc", "gcc", "g++", "c++"].contains(&tool_name.as_str()) {
        return true;
    }
    if tool_name.starts_with("gcc-") {
        return tool_name
            .chars()
            .nth("gcc-".len())
            .unwrap()
            .is_ascii_digit();
    }
    false
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
