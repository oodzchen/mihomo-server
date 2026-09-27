fn main() {
    println!(
        "cargo:rustc-env=MIHOMO_SERVER_TARGET={}",
        std::env::var("TARGET").unwrap()
    );
}
