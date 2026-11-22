//! Decoding against a real ELF with the pinned toolchain's addr2line: app_main's address,
//! found with the same toolchain's nm, decodes to app_main in hello.c. Skips without
//! the toolchain or the firmware (tests/firmware/build.sh).

use std::path::{Path, PathBuf};

use esp32bench_build::backtrace;

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..")
}

#[tokio::test]
async fn decodes_app_main_on_every_chip() {
    let Some(data) = esp32bench_distro::dirs::arduino_data() else {
        eprintln!("skipped: no Arduino data directory");
        return;
    };
    for chip in ["esp32", "esp32s3", "esp32c3"] {
        let elf = root().join(format!(
            "tests/firmware/build/hello-{chip}/esp32bench-hello.elf"
        ));
        let Some(a2l) = backtrace::find_addr2line(&data, chip) else {
            eprintln!("skipped {chip}: the pinned toolchain is not installed");
            continue;
        };
        if !elf.is_file() {
            eprintln!("skipped {chip}: no {}", elf.display());
            continue;
        }
        let nm = a2l.with_file_name(
            a2l.file_name()
                .unwrap()
                .to_string_lossy()
                .replace("addr2line", "nm"),
        );
        let out = std::process::Command::new(&nm).arg(&elf).output().unwrap();
        let syms = String::from_utf8_lossy(&out.stdout);
        let addr = syms
            .lines()
            .find_map(|l| l.strip_suffix(" T app_main"))
            .map(|a| u32::from_str_radix(a, 16).unwrap())
            .expect("app_main in the ELF");
        let frames = backtrace::decode(&a2l, &elf, &[addr]).await.unwrap();
        assert_eq!(
            frames[0].func.as_deref(),
            Some("app_main"),
            "{chip}: {frames:?}"
        );
        assert!(
            frames[0].file.as_deref().unwrap_or("").ends_with("hello.c"),
            "{chip}: {frames:?}"
        );
        assert!(frames[0].line.is_some(), "{chip}: {frames:?}");
    }
}
