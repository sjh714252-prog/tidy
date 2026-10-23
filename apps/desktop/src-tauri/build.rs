fn main() {
    // M11: "정보" shows the commit a release was built from (release.yml
    // sets TIDY_GIT_SHA); `option_env!` in about.rs reads it.
    println!("cargo:rerun-if-env-changed=TIDY_GIT_SHA");
    tauri_build::build()
}
