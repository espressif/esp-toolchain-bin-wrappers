use std::env;
use std::path::Path;
use std::process::Command;

fn main() {
    let target_os = env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    let ptr_width = env::var("CARGO_CFG_TARGET_POINTER_WIDTH").unwrap_or_default();
    let target_env = env::var("CARGO_CFG_TARGET_ENV").unwrap_or_default();
    if target_os != "windows" || ptr_width != "32" || target_env != "gnu" {
        return;
    }

    let dir = env::var("CARGO_MANIFEST_DIR").unwrap();
    let out = env::var("OUT_DIR").unwrap();
    let target = env::var("TARGET").unwrap_or_default();
    let src = Path::new(&dir).join("c/unwind_resume_weak.c");
    let obj = Path::new(&out).join("unwind_resume_weak.o");

    println!("cargo:rerun-if-changed={}", src.display());

    let cc = env::var("CC").unwrap_or_else(|_| {
        let arch = target.split('-').next().unwrap_or("i686");
        format!("{arch}-w64-mingw32-gcc")
    });
    let st = Command::new(&cc)
        .arg("-c")
        .arg(&src)
        .arg("-o")
        .arg(&obj)
        .status()
        .unwrap_or_else(|e| panic!("run {cc}: {e}"));
    assert!(st.success(), "{cc} failed compiling unwind_resume_weak.c");

    println!("cargo:rustc-link-arg={}", obj.display());
}
