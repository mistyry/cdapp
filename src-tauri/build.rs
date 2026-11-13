use std::path::{Path, PathBuf};

const VOLC_ENV: &str = "CDAPP_VOLC_SDK";
const VOLC_DEFAULT: &str = "../third_party/VolcEngineRTC/Windows/3rd/Windows/VolcEngineRTC";
const AGORA_ENV: &str = "CDAPP_AGORA_INCLUDE";
const AGORA_DEFAULT: &str = "../third_party/AgoraAPIExamples/Android/APIExample/agora-stream-encrypt/src/main/cpp/include/agora";

fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let volc_include = volc_bridge();
        agora_bridge(&volc_include);
    }
    tauri_build::build();
}

fn volc_bridge() -> PathBuf {
    println!("cargo:rerun-if-env-changed={VOLC_ENV}");
    let manifest = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"));
    let sdk = std::env::var_os(VOLC_ENV).map_or_else(|| manifest.join(VOLC_DEFAULT), PathBuf::from);
    let include = sdk.join("include");
    let lib_dir = sdk.join("lib/x64");
    for need in [
        include.join("bytertc_video.h"),
        include.join("rtc"),
        lib_dir.join("VolcEngineRTC.lib"),
    ] {
        require(&need, &sdk, VOLC_HINT);
    }

    let native = manifest.join("native");
    for f in ["volc_bridge.cpp", "volc_bridge.h"] {
        println!("cargo:rerun-if-changed={}", native.join(f).display());
    }
    println!(
        "cargo:rerun-if-changed={}",
        lib_dir.join("VolcEngineRTC.lib").display()
    );

    cc::Build::new()
        .cpp(true)
        .std("c++17")
        .file(native.join("volc_bridge.cpp"))
        .include(&include)
        .include(include.join("rtc"))
        .flag("/utf-8")
        .flag("/EHsc")
        .warnings(true)
        .compile("volc_bridge");

    println!("cargo:rustc-link-search=native={}", lib_dir.display());
    println!("cargo:rustc-link-lib=dylib=VolcEngineRTC");
    println!("cargo:rustc-link-lib=dylib=delayimp");
    println!("cargo:rustc-link-lib=dylib=winmm");
    println!("cargo:rustc-link-arg=/DELAYLOAD:VolcEngineRTC.dll");
    native
}

fn agora_bridge(native: &Path) {
    println!("cargo:rerun-if-env-changed={AGORA_ENV}");
    let manifest = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"));
    let include =
        std::env::var_os(AGORA_ENV).map_or_else(|| manifest.join(AGORA_DEFAULT), PathBuf::from);
    require(&include.join("IAgoraRtcEngine.h"), &include, AGORA_HINT);
    for f in ["agora_bridge.cpp", "agora_bridge.h"] {
        println!("cargo:rerun-if-changed={}", native.join(f).display());
    }

    cc::Build::new()
        .cpp(true)
        .std("c++17")
        .file(native.join("agora_bridge.cpp"))
        .include(native)
        .include(&include)
        .flag("/utf-8")
        .flag("/EHsc")
        .warnings(true)
        .compile("agora_bridge");
}

const VOLC_HINT: &str = "运行 git submodule update --init third_party/VolcEngineRTC，或用环境变量 CDAPP_VOLC_SDK 指定 SDK 目录";
const AGORA_HINT: &str = "运行 git submodule update --init third_party/AgoraAPIExamples，或用环境变量 CDAPP_AGORA_INCLUDE 指定头文件目录";

fn require(path: &Path, sdk: &Path, hint: &str) {
    assert!(
        path.exists(),
        "缺少 RTC SDK 文件：{}\n{hint}。当前目录：{}",
        path.display(),
        sdk.display(),
    );
}
