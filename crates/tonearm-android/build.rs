fn main() {
    // Oboe, which plays the sound on Android, is C++. Its runtime is linked
    // in whole, so the app ships no libc++_shared.so beside ours. No kind
    // given: the NDK's linker finds these in its own sysroot.
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("android") {
        println!("cargo:rustc-link-lib=c++_static");
        println!("cargo:rustc-link-lib=c++abi");
    }
}
