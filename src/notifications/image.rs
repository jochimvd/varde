use std::{path::PathBuf, sync::Arc};

use gdk_pixbuf::{Colorspace, InterpType, Pixbuf, glib};

pub(super) const MAX_IMAGE_BYTES: usize = 16 * 1024 * 1024;
pub(super) const MAX_IMAGE_DIMENSION: i32 = 4096;
pub(super) const MAX_IMAGE_AREA: usize = 16_777_216;
const THUMBNAIL_SIZE: i32 = 128;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct Thumbnail {
    pub width: i32,
    pub height: i32,
    pub rowstride: usize,
    pub bytes: Arc<[u8]>,
}

pub(super) fn from_raw(
    width: i32,
    height: i32,
    rowstride: i32,
    has_alpha: bool,
    bits: i32,
    channels: i32,
    bytes: Vec<u8>,
) -> Option<Thumbnail> {
    let (width_usize, height_usize) = valid_dimensions(width, height)?;
    let channels = usize::try_from(channels).ok()?;
    let expected_channels = if has_alpha { 4 } else { 3 };
    let rowstride_usize = usize::try_from(rowstride).ok()?;
    let minimum_stride = width_usize.checked_mul(channels)?;
    let required = rowstride_usize.checked_mul(height_usize)?;
    if bytes.len() > MAX_IMAGE_BYTES
        || bits != 8
        || channels != expected_channels
        || rowstride_usize < minimum_stride
        || required > bytes.len()
    {
        return None;
    }

    let source = Pixbuf::from_bytes(
        &glib::Bytes::from_owned(bytes),
        Colorspace::Rgb,
        has_alpha,
        bits,
        width,
        height,
        rowstride,
    );
    let (target_width, target_height) = target_dimensions(width, height);
    let scaled = if (target_width, target_height) == (width, height) {
        source
    } else {
        source.scale_simple(target_width, target_height, InterpType::Bilinear)?
    };
    tight_rgba(&scaled)
}

pub(super) fn from_path(value: &str) -> Option<Thumbnail> {
    let path = local_path(value)?;
    let metadata = std::fs::metadata(&path).ok()?;
    if !metadata.is_file() || metadata.len() > MAX_IMAGE_BYTES as u64 {
        return None;
    }
    let (_, width, height) = Pixbuf::file_info(&path)?;
    valid_dimensions(width, height)?;
    let (target_width, target_height) = target_dimensions(width, height);
    let pixbuf = Pixbuf::from_file_at_scale(&path, target_width, target_height, false).ok()?;
    tight_rgba(&pixbuf)
}

fn local_path(value: &str) -> Option<PathBuf> {
    if value.starts_with("file://") {
        let (path, hostname) = glib::filename_from_uri(value).ok()?;
        if hostname.as_deref().is_some_and(|host| host != "localhost") {
            return None;
        }
        return path.is_absolute().then_some(path);
    }
    let path = PathBuf::from(value);
    path.is_absolute().then_some(path)
}

fn valid_dimensions(width: i32, height: i32) -> Option<(usize, usize)> {
    if width <= 0 || height <= 0 || width > MAX_IMAGE_DIMENSION || height > MAX_IMAGE_DIMENSION {
        return None;
    }
    let width = usize::try_from(width).ok()?;
    let height = usize::try_from(height).ok()?;
    (width.checked_mul(height)? <= MAX_IMAGE_AREA).then_some((width, height))
}

fn target_dimensions(width: i32, height: i32) -> (i32, i32) {
    if width <= THUMBNAIL_SIZE && height <= THUMBNAIL_SIZE {
        return (width, height);
    }
    if width >= height {
        (THUMBNAIL_SIZE, (height * THUMBNAIL_SIZE / width).max(1))
    } else {
        ((width * THUMBNAIL_SIZE / height).max(1), THUMBNAIL_SIZE)
    }
}

fn tight_rgba(pixbuf: &Pixbuf) -> Option<Thumbnail> {
    let width = pixbuf.width();
    let height = pixbuf.height();
    let (width_usize, height_usize) = valid_dimensions(width, height)?;
    if pixbuf.bits_per_sample() != 8 || !matches!(pixbuf.n_channels(), 3 | 4) {
        return None;
    }
    let channels = usize::try_from(pixbuf.n_channels()).ok()?;
    if pixbuf.has_alpha() != (channels == 4) {
        return None;
    }
    let source_stride = usize::try_from(pixbuf.rowstride()).ok()?;
    let source_row = width_usize.checked_mul(channels)?;
    let source_required = source_stride
        .checked_mul(height_usize.saturating_sub(1))?
        .checked_add(source_row)?;
    let source = pixbuf.read_pixel_bytes();
    let source = source.as_ref();
    if source_stride < source_row || source.len() < source_required {
        return None;
    }

    let rowstride = width_usize.checked_mul(4)?;
    let mut bytes = vec![0; rowstride.checked_mul(height_usize)?];
    for y in 0..height_usize {
        let source = &source[y * source_stride..y * source_stride + source_row];
        let target = &mut bytes[y * rowstride..(y + 1) * rowstride];
        for (source, target) in source
            .chunks_exact(channels)
            .zip(target.chunks_exact_mut(4))
        {
            target[..3].copy_from_slice(&source[..3]);
            target[3] = source.get(3).copied().unwrap_or(255);
        }
    }
    Some(Thumbnail {
        width,
        height,
        rowstride,
        bytes: Arc::from(bytes),
    })
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        sync::atomic::{AtomicU64, Ordering},
    };

    use super::*;

    static NEXT_FILE: AtomicU64 = AtomicU64::new(0);

    struct TestFile(PathBuf);

    impl TestFile {
        fn new(extension: &str) -> Self {
            let number = NEXT_FILE.fetch_add(1, Ordering::Relaxed);
            Self(std::env::temp_dir().join(format!(
                "varde-notification-image-{}-{number}.{extension}",
                std::process::id()
            )))
        }
    }

    impl Drop for TestFile {
        fn drop(&mut self) {
            let _ = fs::remove_file(&self.0);
        }
    }

    fn save_png(width: i32, height: i32) -> TestFile {
        let file = TestFile::new("png");
        let pixbuf = Pixbuf::new(Colorspace::Rgb, true, 8, width, height).unwrap();
        pixbuf.fill(0x336699ff);
        pixbuf.savev(&file.0, "png", &[]).unwrap();
        file
    }

    #[test]
    fn converts_rgb_and_rgba_to_tight_rgba() {
        let rgb = from_raw(2, 1, 6, false, 8, 3, vec![1, 2, 3, 4, 5, 6]).unwrap();
        assert_eq!((rgb.width, rgb.height, rgb.rowstride), (2, 1, 8));
        assert_eq!(rgb.bytes.as_ref(), &[1, 2, 3, 255, 4, 5, 6, 255]);

        let rgba = from_raw(1, 1, 4, true, 8, 4, vec![1, 2, 3, 4]).unwrap();
        assert_eq!(rgba.bytes.as_ref(), &[1, 2, 3, 4]);
    }

    #[test]
    fn removes_source_row_padding() {
        let image = from_raw(
            1,
            2,
            8,
            false,
            8,
            3,
            vec![1, 2, 3, 9, 9, 9, 9, 9, 4, 5, 6, 9, 9, 9, 9, 9],
        )
        .unwrap();
        assert_eq!(image.rowstride, 4);
        assert_eq!(image.bytes.as_ref(), &[1, 2, 3, 255, 4, 5, 6, 255]);
    }

    #[test]
    fn scales_with_aspect_ratio_and_never_upscales() {
        let wide = from_raw(256, 64, 768, false, 8, 3, vec![0; 256 * 64 * 3]).unwrap();
        assert_eq!((wide.width, wide.height), (128, 32));
        let tall = from_raw(64, 256, 192, false, 8, 3, vec![0; 64 * 256 * 3]).unwrap();
        assert_eq!((tall.width, tall.height), (32, 128));
        let small = from_raw(12, 7, 36, false, 8, 3, vec![0; 12 * 7 * 3]).unwrap();
        assert_eq!((small.width, small.height), (12, 7));
    }

    #[test]
    fn rejects_invalid_raw_metadata_and_limits() {
        let invalid = [
            (-1, 1, 4, true, 8, 4, vec![0; 4]),
            (1, 0, 4, true, 8, 4, vec![0; 4]),
            (4097, 1, 16_388, true, 8, 4, vec![]),
            (4096, 4097, 16_384, true, 8, 4, vec![]),
            (2, 1, 7, true, 8, 4, vec![0; 8]),
            (2, 1, 8, true, 16, 4, vec![0; 8]),
            (2, 1, 8, false, 8, 4, vec![0; 8]),
            (2, 1, 8, true, 8, 3, vec![0; 8]),
            (2, 2, 8, true, 8, 4, vec![0; 8]),
        ];
        for (width, height, stride, alpha, bits, channels, bytes) in invalid {
            assert!(from_raw(width, height, stride, alpha, bits, channels, bytes).is_none());
        }
        assert!(from_raw(1, 1, 4, true, 8, 4, vec![0; MAX_IMAGE_BYTES + 1]).is_none());
    }

    #[test]
    fn loads_absolute_paths_and_local_file_uris() {
        let file = save_png(256, 64);
        let path = file.0.to_str().unwrap();
        assert_eq!(
            from_path(path).map(|image| (image.width, image.height)),
            Some((128, 32))
        );
        let uri = glib::filename_to_uri(&file.0, None).unwrap();
        assert_eq!(
            from_path(&uri).map(|image| (image.width, image.height)),
            Some((128, 32))
        );
    }

    #[test]
    fn loaded_thumbnail_is_independent_of_its_source_file() {
        let file = save_png(1, 1);
        let image = from_path(file.0.to_str().unwrap()).unwrap();
        fs::write(&file.0, b"replaced").unwrap();
        assert_eq!(image.bytes.as_ref(), &[0x33, 0x66, 0x99, 0xff]);
        fs::remove_file(&file.0).unwrap();
        assert_eq!(image.bytes.as_ref(), &[0x33, 0x66, 0x99, 0xff]);
    }

    #[test]
    fn rejects_unsupported_paths_and_decode_failures() {
        assert!(from_path("relative.png").is_none());
        assert!(from_path("https://example.com/image.png").is_none());
        assert!(from_path("file://remote.example/image.png").is_none());
        assert!(from_path("data:image/png;base64,nope").is_none());
        assert!(from_path("/definitely/missing/varde.png").is_none());

        let invalid = TestFile::new("png");
        fs::write(&invalid.0, b"not an image").unwrap();
        assert!(from_path(invalid.0.to_str().unwrap()).is_none());
    }

    #[test]
    fn rejects_path_file_size_and_dimension_limits() {
        let large = TestFile::new("png");
        let file = fs::File::create(&large.0).unwrap();
        file.set_len(MAX_IMAGE_BYTES as u64 + 1).unwrap();
        assert!(from_path(large.0.to_str().unwrap()).is_none());

        let dimensions = save_png(4097, 1);
        assert!(from_path(dimensions.0.to_str().unwrap()).is_none());
    }
}
