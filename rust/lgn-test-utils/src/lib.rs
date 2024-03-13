//! legion-test-utils : provides utility functions to help you build integration
//! & unit tests

// crate-specific lint exceptions:
//#![]

use micromegas_tracing::prelude::*;
use std::process::Command;
use std::{
    fs::{self, DirEntry},
    io,
    path::{Path, PathBuf},
};

pub mod jail;

//std::fs::remove_dir_all leaves read-only files and reports an error
#[span_fn]
#[allow(clippy::permissions_set_readonly_false)]
fn force_delete_all(dir: &Path) {
    fn visit_dirs(dir: &Path, cb: &dyn Fn(&DirEntry)) -> io::Result<()> {
        if dir.is_dir() {
            for entry in fs::read_dir(dir)? {
                let entry = entry?;
                let path = entry.path();
                if path.is_dir() {
                    visit_dirs(&path, cb)?;
                }
                cb(&entry);
            }
        }
        Ok(())
    }

    visit_dirs(dir, &|entry| {
        let p = entry.path();
        let meta = entry.metadata().unwrap();
        if meta.is_dir() {
            fs::remove_dir(p).unwrap();
        } else {
            let mut perm = meta.permissions();
            if perm.readonly() {
                perm.set_readonly(false);
                fs::set_permissions(&p, perm).unwrap();
            }
            fs::remove_file(&p).unwrap();
        }
    })
    .unwrap();
}

// create_test_dir creates a directory (or cleans its contents) under the
// `target` folder that will outlive the execution of the test.
#[span_fn]
pub fn create_test_dir(parent_path: &Path, test_name: &str) -> PathBuf {
    let path = parent_path.join(test_name);

    if path.exists() {
        force_delete_all(&path);
    }
    std::fs::create_dir_all(&path).unwrap();
    path
}

// syscall will execute `command` from the `wd` directory and validate that the
// error code matches `should_succeed`
#[span_fn]
pub fn syscall(command: &str, wd: &Path, args: &[&str], should_succeed: bool) {
    info!("{} {}", command, args.join(" "));
    let status = Command::new(command)
        .current_dir(wd)
        .args(args)
        .envs(std::env::vars())
        .status()
        .expect("failed to execute command");
    assert_eq!(status.success(), should_succeed);
}

pub fn rgba_image_diff(img_a: &[u8], img_b: &[u8], width: u32, height: u32) -> f64 {
    let mut max_value: f64 = 0.0;
    let mut current_value: f64 = 0.0;
    for y in 0..height {
        for x in 0..width {
            let left_bound = ((y * width + x) * 4) as usize;
            let right_bound = left_bound + 4;
            let a = &img_a[left_bound..right_bound];
            let b = &img_b[left_bound..right_bound];
            for (a, b) in a.iter().zip(b.iter()) {
                current_value += (f64::from(*a) - f64::from(*b)).abs();
                max_value += f64::from(std::cmp::max(*a, *b));
            }
        }
    }
    current_value / max_value
}

pub fn rgba_per_pixel_image_diff(img_a: &[u8], img_b: &[u8], width: u32, height: u32) -> Vec<u8> {
    let mut diff_image = Vec::new();
    for y in 0..height {
        for x in 0..width {
            let left_bound = ((y * width + x) * 4) as usize;
            let right_bound = left_bound + 4;
            let a = &img_a[left_bound..right_bound];
            let b = &img_b[left_bound..right_bound];
            diff_image.append(
                &mut a
                    .iter()
                    .zip(b.iter())
                    .map(|(a, b)| ((f64::from(*a) - f64::from(*b) + 255.0) / 2.0) as u8)
                    .collect::<Vec<u8>>(),
            );
        }
    }
    diff_image
}
