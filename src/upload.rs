//! 商品图片上传：只认真正的图片（魔数校验），文件名一律重新生成，杜绝路径穿越。

use crate::config::upload_dir;
use rand::Rng;

/// 单张图片体积上限 2MB
pub const MAX_IMAGE_BYTES: usize = 2 * 1024 * 1024;
/// 图片地址最大长度（外链）
pub const MAX_IMAGE_URL: usize = 300;

/// 按文件头判断真实图片类型，返回扩展名
pub fn sniff_ext(b: &[u8]) -> Option<&'static str> {
    if b.len() < 12 {
        return None;
    }
    if b.starts_with(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]) {
        return Some("png");
    }
    if b.starts_with(&[0xFF, 0xD8, 0xFF]) {
        return Some("jpg");
    }
    if b.starts_with(b"GIF87a") || b.starts_with(b"GIF89a") {
        return Some("gif");
    }
    if b.starts_with(b"RIFF") && &b[8..12] == b"WEBP" {
        return Some("webp");
    }
    None
}

/// 保存上传的图片，返回可直接用于 <img src> 的相对地址
pub fn save_image(bytes: &[u8]) -> Result<String, String> {
    if bytes.len() > MAX_IMAGE_BYTES {
        return Err("图片过大，请压缩到 2MB 以内".into());
    }
    let ext = sniff_ext(bytes).ok_or("只支持 PNG / JPG / WEBP / GIF 格式的图片")?;
    let mut rng = rand::thread_rng();
    let rand_part: String = (0..8)
        .map(|_| {
            let c = b"abcdefghijklmnopqrstuvwxyz0123456789";
            c[rng.gen_range(0..c.len())] as char
        })
        .collect();
    let name = format!(
        "{}-{}.{}",
        chrono::Local::now().format("%Y%m%d%H%M%S"),
        rand_part,
        ext
    );
    let path = std::path::Path::new(upload_dir()).join(&name);
    std::fs::write(&path, bytes).map_err(|e| format!("图片保存失败：{e}"))?;
    Ok(format!("/uploads/{name}"))
}

/// 校验图片地址：只允许本站上传路径或 http(s) 外链
pub fn valid_image_ref(s: &str) -> bool {
    if s.is_empty() || s.len() > MAX_IMAGE_URL {
        return false;
    }
    if let Some(name) = s.strip_prefix("/uploads/") {
        return !name.is_empty()
            && !name.contains('/')
            && !name.contains('\\')
            && !name.contains("..");
    }
    s.starts_with("http://") || s.starts_with("https://")
}

/// 删除本站上传的图片文件（外链忽略）；失败不影响主流程
pub fn remove_local(image: &str) {
    let Some(name) = image.strip_prefix("/uploads/") else {
        return;
    };
    if name.is_empty() || name.contains('/') || name.contains('\\') || name.contains("..") {
        return;
    }
    let path = std::path::Path::new(upload_dir()).join(name);
    if let Err(e) = std::fs::remove_file(&path) {
        if e.kind() != std::io::ErrorKind::NotFound {
            tracing::warn!("删除图片 {image} 失败：{e}");
        }
    }
}
