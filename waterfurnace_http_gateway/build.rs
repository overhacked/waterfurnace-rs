use std::process::Command;

fn main() {
    let output = Command::new("git").args(&["describe", "HEAD"]).output().unwrap();
    let git_describe = String::from_utf8(output.stdout).unwrap();
    println!("cargo:rustc-env=GIT_DESCRIBE={}", git_describe);
    println!("cargo:rerun-if-changed=.git/HEAD");
}
