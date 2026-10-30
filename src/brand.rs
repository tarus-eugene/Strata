use gpui::{AssetSource, SharedString};
use std::borrow::Cow;

pub struct BrandAssets;

impl AssetSource for BrandAssets {
    fn load(&self, path: &str) -> anyhow::Result<Option<Cow<'static, [u8]>>> {
        Ok(match path {
            "brand/mark.png" => Some(Cow::Borrowed(include_bytes!("../assets/brand/mark.png"))),
            _ => None,
        })
    }

    fn list(&self, _: &str) -> anyhow::Result<Vec<SharedString>> {
        Ok(vec!["brand/mark.png".into()])
    }
}
