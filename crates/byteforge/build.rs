//! Windows: embed the version details shown in the exe's file properties. Unsigned executables without
//! them look more suspicious to antivirus heuristics. GPUI embeds the application manifest itself.

fn main() {
    #[cfg(windows)]
    {
        let mut res = winresource::WindowsResource::new();
        res.set("ProductName", "ByteForge")
            .set("FileDescription", "ByteForge - IDE for C#")
            .set("CompanyName", "DevStarByte")
            .set("LegalCopyright", "DevStarByte, GPL-2.0-only")
            .set("OriginalFilename", "byteforge.exe")
            .set("InternalName", "byteforge");
        res.compile().expect("embedding the Windows version resource");
    }
}
