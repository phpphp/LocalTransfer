//! 应用资产：自有 icons/ 优先（测速等自绘图标），回落 gpui-kit 组件库图标包。
//! gpui 的 AssetSource 只能挂一个——组合两层即可让 Icon::new(...).path("icons/speedtest.svg")
//! 与组件库内置图标共存。

use gpui_kit::{AssetSource, Result, SharedString};
use std::borrow::Cow;

#[derive(rust_embed::RustEmbed)]
#[folder = "../../assets"]
#[include = "icons/**/*"]
struct OwnAssets;

pub struct AppAssets;

impl AssetSource for AppAssets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        if path.is_empty() {
            return Ok(None);
        }
        // 自有资产优先
        if let Some(f) = OwnAssets::get(path) {
            return Ok(Some(f.data));
        }
        // 回落组件库图标包
        gpui_kit::assets::Assets.load(path)
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        let mut out: Vec<SharedString> = OwnAssets::iter()
            .filter(|p| p.starts_with(path))
            .map(Into::into)
            .collect();
        out.extend(gpui_kit::assets::Assets.list(path)?);
        Ok(out)
    }
}
