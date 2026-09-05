fn main() {
    // 图标变了就重新链接资源
    println!("cargo:rerun-if-changed=../../assets/icon.ico");
    #[cfg(target_os = "windows")]
    embed_icon();
}

/// Windows 资源（图标/版本信息）。cfg 属性门控（不是 cfg!）：
/// 后者在非 Windows 上仍会解析 winresource 路径导致编译失败。
#[cfg(target_os = "windows")]
fn embed_icon() {
    let mut res = winresource::WindowsResource::new();
    res.set_icon("../../assets/icon.ico");
    res.set("ProductName", "LocalTransfer");
    res.set("FileDescription", "LocalTransfer - LAN file & text transfer");
    res.set("LegalCopyright", "");
    if let Err(e) = res.compile() {
        println!("cargo:warning=嵌入图标失败（不影响功能）: {e}");
    }
}
