//! Windows only: the icon, the version information (VERSIONINFO) and the application manifest in
//! `mailalm.exe`. Without them the program is anonymous to Windows: no name in Explorer or the
//! taskbar, and a profile that antivirus heuristics distrust.
//!
//! Nothing happens on other targets. A missing resource compiler is a warning, unless
//! `MAILALM_REQUIRE_WINRES` is set (the CI build), which makes it an error.

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=windows/mailalm.manifest");
    println!("cargo:rerun-if-changed=windows/mailalm.ico");
    println!("cargo:rerun-if-env-changed=MAILALM_REQUIRE_WINRES");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let mut res = winresource::WindowsResource::new();
    res.set_icon("windows/mailalm.ico")
        .set_manifest(include_str!("windows/mailalm.manifest"))
        .set("ProductName", "MailALM")
        .set("FileDescription", "MailALM mail and calendar")
        .set("CompanyName", "quackez-dev")
        .set("LegalCopyright", "Copyright (c) 2026 quackez-dev. MIT OR Apache-2.0.")
        .set("OriginalFilename", "mailalm.exe")
        .set("InternalName", "mailalm");
    if let Err(e) = res.compile() {
        if std::env::var_os("MAILALM_REQUIRE_WINRES").is_some() {
            eprintln!("embedding the Windows resources failed: {e}");
            std::process::exit(1);
        }
        println!("cargo:warning=mailalm.exe built without its icon, version and manifest: {e}");
    }
}
