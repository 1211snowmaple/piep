/// Pixiv's system gate icons are valid image files, but are not the creator's
/// cover or illustration. Use the same rule in acquisition, audits and readers.
pub(crate) fn is_pixiv_placeholder_image(url: &str) -> bool {
    url.contains("/common/images/limit_")
}
