//! Windows: embed the app icon and the version details shown in the exe's file properties. GPUI loads icon
//! resource 1 for its window class, so the icon also appears in the taskbar and title bar. Unsigned
//! executables without version details look more suspicious to antivirus heuristics. GPUI embeds the
//! application manifest itself.

fn main() {
    #[cfg(windows)]
    {
        const ICON: &str = "../../packaging/windows/byteforge.ico";
        println!("cargo:rerun-if-changed={ICON}");
        let mut res = winresource::WindowsResource::new();
        res.set_icon_with_id(ICON, "1")
            .set("ProductName", "ByteForge")
            .set("FileDescription", "ByteForge - IDE for C#")
            .set("CompanyName", "DevStarByte")
            .set("LegalCopyright", "DevStarByte, GPL-2.0-only")
            .set("OriginalFilename", "byteforge.exe")
            .set("InternalName", "byteforge");
        res.compile().expect("embedding the Windows icon and version resource");
    }
}
