use std::path::{Path, PathBuf};

/// 火山 RTC SDK 的头文件与导入库位置，环境变量可覆盖。默认为子模块 `third_party/VolcEngineRTC` 中的 SDK 目录。
const VOLC_ENV: &str = "CDAPP_VOLC_SDK";
const VOLC_DEFAULT: &str = "../third_party/VolcEngineRTC/Windows/3rd/Windows/VolcEngineRTC";

fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        volc_bridge();
    }
    tauri_build::build();
}

/// 编译火山 SDK 桥接。头文件或导入库缺失时构建失败。
fn volc_bridge() {
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
        require(&need, &sdk);
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
}

fn require(path: &Path, sdk: &Path) {
    assert!(
        path.exists(),
        "缺少火山 RTC SDK 文件：{}\n运行 git submodule update --init third_party/VolcEngineRTC，或用环境变量 {VOLC_ENV} 指定 SDK 目录。当前目录：{}",
        path.display(),
        sdk.display(),
    );
}
