extern crate libc;

use std::env;
use std::ffi::CString;
use std::iter::once;
use std::path::Path;
use std::ptr::null;

const CONFIG_ENV_NAME: &str = "XTENSA_GNU_CONFIG";
const XTENSA_TOOLCHAIN_PREFIX: &str = "xtensa-esp-elf-";
const XTENSA_TOOL_PARSE_ERROR: &str = "Called tool must have pattern \"xtensa-esp*-elf-*\"";

fn main() {
    let argv_0 = env::args().nth(0).expect("Get argv[0]");
    let wrapper_name = Path::new(&argv_0)
        .file_name()
        .expect("Filename in argv[0]")
        .to_str()
        .unwrap();

    let mut chip = "";
    let mut tool_name = Vec::<&str>::new();
    for (i, s) in wrapper_name.split("-").enumerate() {
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

    let wrapper_path = env::current_exe().expect("Get exec full path");
    let bin_dir = wrapper_path
        .parent()
        .expect("Executable must be in some directory");

    /* Get tool path */
    let exec_path = bin_dir.join(format!("{}{}", XTENSA_TOOLCHAIN_PREFIX, tool_name));
    let exec = exec_path.as_path().display().to_string();
    assert!(
        exec_path.try_exists().unwrap(),
        "Tool {} is not exist",
        exec
    );
    let exec = CString::new(exec).unwrap();

    let dynconfig_filename = format!("xtensa_{}.so", chip);
    /* Get dynconfig path */
    let dynconfig_path = bin_dir
        .parent()
        .expect("Toolchain must be in some directory")
        .join("lib")
        .as_path()
        .join(dynconfig_filename.clone());
    let dynconfig = dynconfig_path.as_path().display().to_string();
    assert!(
        dynconfig_path.try_exists().unwrap(),
        "Dynconfig for target {} is not exist ({})",
        chip,
        dynconfig
    );

    /* Set XTENSA_GNU_CONFIG env variable */
    env::set_var(CONFIG_ENV_NAME, dynconfig);

    let mut argv: Vec<CString> = std::env::args()
        .enumerate()
        .map(|(i, arg)| {
            if i == 0 {
                /* The first arg must contain app name */
                exec.clone()
            } else {
                CString::new(arg).unwrap()
            }
        })
        .collect();

    if is_compiler(tool_name) {
        /* Need to add mdynconfig option for using the right multilib instance */
        let dynconfig_option = CString::new(format!("-mdynconfig={}", dynconfig_filename)).unwrap();
        argv.insert(1, dynconfig_option);
    }

    let argv: Vec<_> = argv
        .iter()
        .map(|x| x.as_ptr())
        .chain(once(null()))
        .collect();

    unsafe { libc::execv(exec.as_ptr(), argv.as_ptr()) };
}

fn is_compiler(tool_name: String) -> bool {
    /* consider tools:
     * xtensa-esp-elf-cc
     * xtensa-esp-elf-gcc
     * xtensa-esp-elf-g++
     * xtensa-esp-elf-c++
     * xtensa-esp-elf-gcc-13.1.0
     */
    if ["cc", "gcc", "g++", "c++"].contains(&tool_name.as_str()) {
        return true;
    }
    if tool_name.starts_with("gcc-") {
        return tool_name.chars().nth("gcc-".len()).unwrap().is_digit(10);
    }
    return false;
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
