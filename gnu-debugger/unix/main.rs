use lazy_static::lazy_static;
use std::env;
use std::ffi::CString;
use std::iter::once;
use std::process::{Command, Output, Stdio};
use std::ptr::null;

const PYTHON_EXECUTABLE: &str = "python3";
const PYTHON_GET_VERSION: &str =
    "import sys; print('{}.{}'.format(sys.version_info.major, sys.version_info.minor))";
const PYTHON_GET_PYTHONHOME: &str = "import sys; print(sys.base_prefix)";
const PYTHON_GET_PYTHONPATH: &str = "import os, sys; print(os.pathsep.join(sys.path[1:]))";

const PYTHON_GET_LIBDIR: &str = if cfg!(unix) {
    "import sys, os, sysconfig; print(os.path.join(sys.base_prefix, 'lib'))"
} else if cfg!(windows) {
    "import sys; print(sys.base_prefix)"
} else {
    panic!("OS is not supported")
};

const PYTHON_LD_LIBRARY_PATH_VARIABLE: &str = if cfg!(all(unix, not(target_os = "macos"))) {
    "LD_LIBRARY_PATH"
} else if cfg!(target_os = "macos") {
    "DYLD_LIBRARY_PATH"
} else if cfg!(windows) {
    "PATH"
} else {
    panic!("OS is not supported")
};

const PYTHON_ENV_DELIMETER: &str = if cfg!(windows) { ";" } else { ":" };
const EXE_EXTENSION: &str = if cfg!(windows) { ".exe" } else { "" };
const GDB_NOPYTHON_POSTFIX: &str = "no-python";

lazy_static! {
    static ref ESP_DEBUG_TRACE: bool = match env::var("ESP_DEBUG_TRACE") {
        Ok(_) => true,
        Err(_) => false,
    };
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

fn exec_python_script(script: &str) -> Result<String, String> {
    let mut command = Command::new(PYTHON_EXECUTABLE);
    command.arg("-c").arg(script);

    let output: Output = match command.output() {
        Ok(o) => o,
        Err(e) => return Err(format!("Failed to execute process: {}", e)),
    };

    if !output.status.success() {
        esp_debug_trace!(
            "Error {:#?} while executing Python script:\n\t{}",
            String::from_utf8_lossy(&output.stderr),
            script
        );
        return Err("Python script execution failed".to_string());
    }

    return Ok(String::from_utf8_lossy(&output.stdout).trim().to_string());
}

fn add_to_environment(var_name: &str, new_value: String, append: bool) {
    let mut value = new_value.clone();
    if append {
        if let Ok(old_value) = env::var(var_name) {
            if !old_value.is_empty() {
                value = format!("{}{}{}", new_value, PYTHON_ENV_DELIMETER, old_value);
            }
        }
    }
    esp_debug_trace!("export {}={}", var_name, value);
    env::set_var(var_name, value);
}

fn update_environment_variables() {
    esp_debug_trace!("Update environment variables ...");
    add_to_environment(
        PYTHON_LD_LIBRARY_PATH_VARIABLE,
        exec_python_script(PYTHON_GET_LIBDIR).unwrap(),
        true,
    );
    add_to_environment(
        "PYTHONHOME",
        exec_python_script(PYTHON_GET_PYTHONHOME).unwrap(),
        false,
    );
    add_to_environment(
        "PYTHONPATH",
        exec_python_script(PYTHON_GET_PYTHONPATH).unwrap(),
        true,
    );
}

fn get_exec_argv(no_python: bool) -> Vec<String> {
    esp_debug_trace!("Building base argv to execute GDB ...");
    let wrapper_path = env::current_exe().expect("Get exec full path");
    let wrapper_name = wrapper_path
        .file_name()
        .expect("Filename in argv[0]")
        .to_str()
        .unwrap();
    let bin_dir = wrapper_path
        .parent()
        .expect("Executable must be in some directory");

    let mut arch = "";
    let mut chip = "";
    for (i, s) in wrapper_name.split("-").enumerate() {
        match i {
            0 => arch = s,
            1 => chip = s,
            _ => (),
        }
    }

    if arch == "xtensa" {
        let dynconfig_path = bin_dir
            .parent()
            .expect("Executable must be in some directory")
            .join("lib")
            .join(format!("xtensa_{}.so", chip))
            .as_path()
            .display()
            .to_string();
        add_to_environment("XTENSA_GNU_CONFIG", dynconfig_path, false);
        chip = "esp";
    }
    let python_version = if no_python {
        GDB_NOPYTHON_POSTFIX.to_string()
    } else {
        exec_python_script(PYTHON_GET_VERSION).unwrap_or_else(|_| GDB_NOPYTHON_POSTFIX.to_string())
    };
    let exec_path = bin_dir.join(format!(
        "{}-{}-elf-gdb-{}{}",
        arch, chip, python_version, EXE_EXTENSION
    ));

    /* If gdb with-python but no binary found switch to gdb-no-python.
     * Assume that gdb-no-python is exist always */
    let exec_exist = exec_path.try_exists().unwrap();
    esp_debug_trace!("Executable {:?} exist: {}", exec_path, exec_exist);
    let exec_path = if !no_python && !exec_exist {
        bin_dir.join(format!(
            "{}-{}-elf-gdb-{}{}",
            arch,
            chip,
            GDB_NOPYTHON_POSTFIX.to_string(),
            EXE_EXTENSION
        ))
    } else {
        exec_path
    };
    assert!(
        exec_path.try_exists().unwrap(),
        "Executable {:?} is not exist",
        exec_path
    );
    let exec_path = exec_path.as_path().display().to_string();

    let argv = vec![exec_path];
    esp_debug_trace!("Base argv is: {:?}", argv);
    return argv;
}

fn exec_gdb_test(mut argv: Vec<String>) -> bool {
    argv.extend(vec!["--batch-silent".to_string()]);

    esp_debug_trace!("Test execution of GDB with argv: {:?}", argv);
    match Command::new(argv.get(0).unwrap())
        .args(&argv[1..])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
    {
        Ok(s) => {
            if s.success() {
                esp_debug_trace!("Test execution of GDB is OK!");
                return true;
            } else {
                esp_debug_trace!(
                    "Test execution of GDB has non-zero exit code: {}",
                    s.code().unwrap_or(-1)
                );
                return false;
            }
        }
        Err(e) => {
            esp_debug_trace!("GDB executed with error {}", e);
            return false;
        }
    };
}

fn exec_gdb(mut argv: Vec<String>) {
    argv.extend(std::env::args().peekable().skip(1));
    esp_debug_trace!("Execute GDB: {:?}", argv);

    // Convert Vec<String> into Vec<CString>
    let c_argv: Vec<CString> = argv
        .iter()
        .map(|x| CString::new(x.as_bytes()).unwrap())
        .collect();
    // add null ptr to the end of vector
    let c_argv: Vec<_> = c_argv
        .iter()
        .map(|x| x.as_ptr())
        .chain(once(null()))
        .collect();

    let exec = c_argv.get(0).expect("app in argv[0]").clone();
    unsafe { libc::execv(exec, c_argv.as_ptr()) };
    println!(
        "execv errno ({})",
        std::io::Error::last_os_error().raw_os_error().unwrap()
    );
    unreachable!();
}

fn main() {
    let mut argv = get_exec_argv(false);
    let exec = argv.get(0).expect("app in argv[0]");
    if !exec.contains(GDB_NOPYTHON_POSTFIX) {
        esp_debug_trace!("Trying to execute GDB-with-Python");
        update_environment_variables();
        if !exec_gdb_test(argv.clone()) {
            argv = get_exec_argv(true); // fallback to no-python gdb
        }
    }
    exec_gdb(argv);
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
