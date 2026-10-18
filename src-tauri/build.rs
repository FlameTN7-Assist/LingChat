fn main() {
    // Linux 桌面：让动态加载器在可执行文件所在目录查找随包分发的动态库。
    // ort 的 webgpu 后端依赖 libwebgpu_dawn.so（Dawn dylib，无法静态链接），
    // 它不在系统库路径中；便携包把它放在 exe 旁边，若二进制没有 $ORIGIN
    // RUNPATH，加载器只搜系统路径，直接报找不到共享库。
    //
    // 注意：build script 中 cfg!(target_os) 判断的是**宿主**平台，交叉编译
    // （如在 Linux 上构建 Android）会误判，因此必须读 CARGO_CFG_TARGET_OS。
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("linux") {
        println!("cargo:rustc-link-arg=-Wl,-rpath,$ORIGIN");
    }

    tauri_build::build();
}
