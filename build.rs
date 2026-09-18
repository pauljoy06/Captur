use std::{
    env,
    ffi::OsString,
    path::{Path, PathBuf},
    process::Command,
};

fn main() {
    println!("cargo:rerun-if-changed=assets/captur.rc");
    println!("cargo:rerun-if-changed=assets/captur.ico");

    let target = env::var("TARGET").unwrap_or_default();
    if !target.contains("windows") {
        return;
    }

    let manifest_dir = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").unwrap());
    let output = PathBuf::from(env::var_os("OUT_DIR").unwrap()).join("captur.res");
    let source = manifest_dir.join("assets/captur.rc");
    let include_dir = manifest_dir.join("assets");

    compile_resource(&source, &include_dir, &output);
    println!("cargo:rustc-link-arg={}", output.display());
}

fn compile_resource(source: &Path, include_dir: &Path, output: &Path) {
    let configured = env::var_os("RC");
    let compilers: Vec<OsString> = configured
        .map(|compiler| vec![compiler])
        .unwrap_or_else(|| {
            if cfg!(windows) {
                vec![OsString::from("rc.exe"), OsString::from("llvm-rc")]
            } else {
                vec![OsString::from("llvm-rc")]
            }
        });

    let mut unavailable = Vec::new();
    for compiler in compilers {
        let result = Command::new(&compiler)
            .arg("/FO")
            .arg(output)
            .arg("/I")
            .arg(include_dir)
            .arg(source)
            .status();

        match result {
            Ok(status) if status.success() => return,
            Ok(status) => panic!(
                "Windows resource compiler {:?} failed with {status}",
                compiler
            ),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                unavailable.push(compiler)
            }
            Err(error) => panic!(
                "could not run Windows resource compiler {:?}: {error}",
                compiler
            ),
        }
    }

    panic!(
        "no Windows resource compiler was found (tried {unavailable:?}); install LLVM's llvm-rc, run from a Visual Studio developer shell, or set RC"
    );
}
