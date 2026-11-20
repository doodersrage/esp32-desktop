//! A real ESP-IDF build of tests/firmware/hello (an ESP-IDF project) for the S3, with
//! the installed ESP-IDF. Skips without one. The build directory and sdkconfig go to
//! the cache, so the project folder must stay as it was.
use std::path::{Path, PathBuf};

use esp32bench_build::idf;

#[tokio::test]
async fn builds_a_local_idf_project_into_one_flash_image() {
    let Some(install) = idf::detect() else {
        eprintln!("skipping: ESP-IDF is not installed");
        return;
    };
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../tests/firmware/hello");
    let before: Vec<_> = std::fs::read_dir(&root)
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    let project = idf::check_project(&root).unwrap();
    assert_eq!(project.name, "esp32bench-hello");
    // A cache that outlives the test, so reruns build incrementally.
    let cache = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("idf-builds");
    let plan = idf::plan(&project, "esp32s3", &install).unwrap();
    let dirs = idf::Dirs::new(&cache, &plan);
    let mut lines = 0;
    let out = idf::run(&plan, &dirs, |_, _| lines += 1).await.unwrap();
    check_build(&out, lines);

    // The way Windows builds run (no shell: ESP-IDF's Python and idf_tools.py export),
    // checked here too, into its own cache.
    if idf::python_env(&install).is_some() {
        let cache = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("idf-builds-direct");
        let dirs = idf::Dirs::new(&cache, &plan);
        let mut lines = 0;
        let out = idf::run_with(&plan, &dirs, idf::Activation::Direct, |_, _| lines += 1)
            .await
            .unwrap();
        check_build(&out, lines);
    }

    let after: Vec<_> = std::fs::read_dir(&root)
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    assert_eq!(before, after, "the project folder was written to");
}

fn check_build(out: &esp32bench_build::Output, lines: usize) {
    assert!(lines > 10);
    let image = std::fs::read(&out.image).unwrap();
    assert_eq!(image.len(), 4 * 1024 * 1024, "padded to 4 MB");
    // The S3's bootloader sits at 0: an image header (0xE9) there.
    assert_eq!(image[0], 0xE9);
    assert!(Path::new(&out.elf).is_file());
}
