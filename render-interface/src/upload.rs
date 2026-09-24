//! Optional byte-upload conveniences for preparation callbacks. They record
//! staging copies into the supplied encoder and never submit work themselves.

use crate::{GpuPrepareContext, PrepareError, PrepareResult, TextureDescriptor, TextureTarget};

/// Record a byte upload without Queue access; submit remains renderer-owned.
/// Empty data is a no-op. Nonempty data must fit and be copy-aligned; this helper
/// may write a prefix, but the generator must initialize its complete output.
pub fn upload_buffer(
    gpu: &mut GpuPrepareContext<'_>,
    target: &wgpu::Buffer,
    bytes: &[u8],
) -> PrepareResult {
    use wgpu::util::DeviceExt;
    let size = buffer_upload_size(target.size(), bytes.len())?;
    if size == 0 {
        return Ok(());
    }
    if !target.usage().contains(wgpu::BufferUsages::COPY_DST) {
        return Err("buffer upload target lacks COPY_DST".into());
    }
    if size > gpu.device.limits().max_buffer_size {
        return Err("buffer upload exceeds device buffer limit".into());
    }
    let staging = gpu
        .device
        .create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("source upload"),
            contents: bytes,
            usage: wgpu::BufferUsages::COPY_SRC,
        });
    gpu.encoder
        .copy_buffer_to_buffer(&staging, 0, target, 0, size);
    Ok(())
}
/// Tightly packed rows, padded here to WebGPU's copy alignment.
/// Supported formats are R8Unorm, Rgba8Unorm, Rgba8UnormSrgb and Rgba16Float.
/// Zero dimensions, invalid byte counts and unrepresentable staging sizes fail
/// before any GPU commands are recorded; no conversion or rescaling is performed.
pub fn upload_texture(
    gpu: &mut GpuPrepareContext<'_>,
    target: &TextureTarget<'_>,
    bytes: &[u8],
) -> PrepareResult {
    use wgpu::util::DeviceExt;
    let layout = texture_upload_layout(
        target.desc,
        bytes.len(),
        gpu.device.limits().max_buffer_size,
    )?;
    let [w, h] = target.desc.size;
    if target.texture.size()
        != (wgpu::Extent3d {
            width: w,
            height: h,
            depth_or_array_layers: 1,
        })
        || target.texture.format() != target.desc.format
        || target.texture.dimension() != wgpu::TextureDimension::D2
        || target.texture.mip_level_count() != 1
        || target.texture.sample_count() != 1
        || !target
            .texture
            .usage()
            .contains(wgpu::TextureUsages::COPY_DST)
    {
        return Err("texture upload target does not match its descriptor or lacks COPY_DST".into());
    }
    let data = padded_texture_data(bytes, &layout)?;
    let staging = gpu
        .device
        .create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("texture source upload"),
            contents: &data,
            usage: wgpu::BufferUsages::COPY_SRC,
        });
    gpu.encoder.copy_buffer_to_texture(
        wgpu::TexelCopyBufferInfo {
            buffer: &staging,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(layout.padded_row),
                rows_per_image: Some(h),
            },
        },
        target.texture.as_image_copy(),
        wgpu::Extent3d {
            width: w,
            height: h,
            depth_or_array_layers: 1,
        },
    );
    Ok(())
}

fn buffer_upload_size(target_size: u64, byte_len: usize) -> Result<u64, PrepareError> {
    let size = u64::try_from(byte_len).map_err(|_| "buffer upload length overflow")?;
    if size > target_size || size % wgpu::COPY_BUFFER_ALIGNMENT != 0 {
        return Err("invalid buffer upload length/alignment".into());
    }
    Ok(size)
}

struct TextureUploadLayout {
    row: usize,
    padded_row: u32,
    staging_size: usize,
}

fn texture_upload_layout(
    desc: &TextureDescriptor,
    byte_len: usize,
    max_buffer_size: u64,
) -> Result<TextureUploadLayout, PrepareError> {
    let bpp = match desc.format {
        wgpu::TextureFormat::R8Unorm => 1,
        wgpu::TextureFormat::Rgba8Unorm | wgpu::TextureFormat::Rgba8UnormSrgb => 4,
        wgpu::TextureFormat::Rgba16Float => 8,
        _ => return Err("unsupported upload format".into()),
    };
    let [w, h] = desc.size;
    if w == 0 || h == 0 {
        return Err("texture upload dimensions must be nonzero".into());
    }
    let row = w.checked_mul(bpp).ok_or("row size overflow")?;
    let alignment = wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
    let padded_row = row
        .checked_next_multiple_of(alignment)
        .ok_or("row padding overflow")?;
    let byte_len = u64::try_from(byte_len).map_err(|_| "texture upload length overflow")?;
    if byte_len != u64::from(row) * u64::from(h) {
        return Err("invalid texture upload length".into());
    }
    let staging_size = u64::from(padded_row) * u64::from(h);
    if staging_size > max_buffer_size {
        return Err("texture upload exceeds device buffer limit".into());
    }
    // Vec allocations cannot exceed isize::MAX even on a 64-bit host. Check
    // before narrowing to usize; padded rows can overflow a 32-bit allocation
    // even when the original tightly packed byte slice fits that address space.
    let staging_size = usize::try_from(staging_size)
        .ok()
        .filter(|size| *size <= isize::MAX as usize)
        .ok_or("texture staging allocation size overflow")?;
    Ok(TextureUploadLayout {
        row: usize::try_from(row).map_err(|_| "texture row size overflow")?,
        padded_row,
        staging_size,
    })
}

fn padded_texture_data(
    bytes: &[u8],
    layout: &TextureUploadLayout,
) -> Result<Vec<u8>, PrepareError> {
    let mut data = Vec::new();
    data.try_reserve_exact(layout.staging_size)?;
    data.resize(layout.staging_size, 0);
    for (source, target) in bytes
        .chunks_exact(layout.row)
        .zip(data.chunks_exact_mut(layout.padded_row as usize))
    {
        target[..layout.row].copy_from_slice(source);
    }
    Ok(data)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn buffer_upload_rejects_unaligned_or_oversized_data_but_allows_empty() {
        assert_eq!(buffer_upload_size(0, 0).expect("empty upload is valid"), 0);
        assert_eq!(buffer_upload_size(8, 4).expect("aligned prefix fits"), 4);
        assert!(buffer_upload_size(8, 3).is_err());
        assert!(buffer_upload_size(8, 12).is_err());
    }

    #[test]
    fn texture_rows_preserve_pixels_and_zero_the_padding() {
        let desc = TextureDescriptor::new([3, 2], wgpu::TextureFormat::Rgba8Unorm);
        let bytes: Vec<u8> = (0..24).collect();
        let layout = texture_upload_layout(&desc, bytes.len(), 512)
            .expect("two short rows fit the staging buffer");
        let padded = padded_texture_data(&bytes, &layout).expect("small CPU allocation succeeds");
        assert_eq!(padded.len(), 512);
        assert_eq!(&padded[..12], &bytes[..12]);
        assert_eq!(&padded[256..268], &bytes[12..]);
        assert!(padded[12..256].iter().all(|b| *b == 0));
        assert!(padded[268..].iter().all(|b| *b == 0));
    }

    #[test]
    fn aligned_rows_need_no_extra_padding_for_each_supported_format() {
        for (format, width) in [
            (wgpu::TextureFormat::R8Unorm, 256),
            (wgpu::TextureFormat::Rgba8Unorm, 64),
            (wgpu::TextureFormat::Rgba8UnormSrgb, 64),
            (wgpu::TextureFormat::Rgba16Float, 32),
        ] {
            let desc = TextureDescriptor::new([width, 2], format);
            let bytes = vec![19; 512];
            let layout = texture_upload_layout(&desc, bytes.len(), 512)
                .expect("aligned rows occupy exactly their packed byte count");
            assert_eq!(
                padded_texture_data(&bytes, &layout).expect("small CPU allocation succeeds"),
                bytes
            );
        }
    }

    #[test]
    fn empty_extents_bad_lengths_and_unsupported_formats_fail_on_cpu() {
        for size in [[0, 1], [1, 0], [0, 0]] {
            let desc = TextureDescriptor::new(size, wgpu::TextureFormat::R8Unorm);
            assert!(texture_upload_layout(&desc, 0, 1024).is_err());
        }
        let desc = TextureDescriptor::new([2, 2], wgpu::TextureFormat::R8Unorm);
        assert!(texture_upload_layout(&desc, 3, 1024).is_err());
        assert!(texture_upload_layout(&desc, 5, 1024).is_err());
        let unsupported = TextureDescriptor::new([1, 1], wgpu::TextureFormat::Depth32Float);
        assert!(texture_upload_layout(&unsupported, 4, 1024).is_err());
    }

    #[test]
    fn row_overflow_and_padded_device_limit_fail_without_allocating() {
        let large = TextureDescriptor::new([u32::MAX, 1], wgpu::TextureFormat::Rgba16Float);
        assert!(texture_upload_layout(&large, 0, u64::MAX).is_err());
        let padding = TextureDescriptor::new([u32::MAX, 1], wgpu::TextureFormat::R8Unorm);
        assert!(texture_upload_layout(&padding, 0, u64::MAX).is_err());
        let tiny = TextureDescriptor::new([1, 2], wgpu::TextureFormat::R8Unorm);
        // Two source bytes still require two 256-byte staging rows.
        assert!(texture_upload_layout(&tiny, 2, 511).is_err());
        assert!(texture_upload_layout(&tiny, 2, 512).is_ok());
    }

    #[test]
    fn staging_larger_than_addressable_allocation_fails_without_allocating() {
        // Width one keeps the logical bytes representable on 32-bit hosts while
        // padding overflows their address space. On 64-bit hosts the wide image
        // fits usize, but its staging allocation exceeds Vec's isize::MAX bound.
        let size = if usize::BITS == 32 {
            [1, u32::MAX]
        } else {
            [0x8000_0100, u32::MAX]
        };
        let desc = TextureDescriptor::new(size, wgpu::TextureFormat::R8Unorm);
        let byte_len = usize::try_from(u64::from(size[0]) * u64::from(size[1]))
            .expect("the declared packed byte count fits this architecture");
        let error = texture_upload_layout(&desc, byte_len, u64::MAX)
            .err()
            .expect("oversized CPU staging must be rejected before allocation");
        assert!(error.to_string().contains("allocation size overflow"));
    }
}
