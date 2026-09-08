fn main() {
    // 오류 창에 "어느 릴리즈의 인스톨러인지"를 실어야 원격 진단이 된다.
    // installer 자신의 버전(0.1.0)은 릴리즈 이름과 무관해 쓸 수 없으므로,
    // 릴리즈 이름이 되는 루트 패키지의 version을 굽는다.
    println!("cargo:rerun-if-changed=../Cargo.toml");
    let root = std::fs::read_to_string("../Cargo.toml").expect("workspace root Cargo.toml");
    let version = root
        .lines()
        .find_map(|l| l.strip_prefix("version = "))
        .expect("root Cargo.toml must have a top-level `version = `")
        .trim()
        .trim_matches('"');
    println!("cargo:rustc-env=INNO_CREED_RELEASE={version}");

    #[cfg(target_os = "windows")]
    {
        winresource::WindowsResource::new()
            .set_icon("assets/icon.ico")
            .compile()
            .expect("failed to embed installer.exe icon resource");
    }
}
