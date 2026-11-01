//! Shared, dependency-free Finder artwork for PE resources, the window, tray and installer.
pub fn rgba(size: u32) -> Vec<u8> {
    let mut pixels = vec![0; (size * size * 4) as usize];
    let samples = 4.;
    for y in 0..size {
        for x in 0..size {
            let mut total = [0_f32; 4];
            for sy in 0..4 {
                for sx in 0..4 {
                    let px = (x as f32 + (sx as f32 + 0.5) / samples) * 32. / size as f32;
                    let py = (y as f32 + (sy as f32 + 0.5) / samples) * 32. / size as f32;
                    let qx = (px - 16.).abs() - 8.;
                    let qy = (py - 16.).abs() - 8.;
                    let inside = qx.max(0.).hypot(qy.max(0.)) <= 8.;
                    if !inside {
                        continue;
                    }
                    let ring = ((px - 14.4).hypot(py - 14.4) - 5.34).abs() <= 0.91;
                    let a = (18.14, 18.14);
                    let b = (22.4, 22.4);
                    let t = (((px - a.0) * (b.0 - a.0) + (py - a.1) * (b.1 - a.1))
                        / ((b.0 - a.0) * (b.0 - a.0) + (b.1 - a.1) * (b.1 - a.1)))
                        .clamp(0., 1.);
                    let handle =
                        (px - (a.0 + t * (b.0 - a.0))).hypot(py - (a.1 + t * (b.1 - a.1))) <= 0.91;
                    let color = if ring || handle {
                        [111., 180., 255.]
                    } else {
                        [37., 65., 99.]
                    };
                    for c in 0..3 {
                        total[c] += color[c];
                    }
                    total[3] += 255.;
                }
            }
            let offset = ((y * size + x) * 4) as usize;
            let alpha = total[3] / 16.;
            if alpha > 0. {
                for c in 0..3 {
                    pixels[offset + c] = (total[c] * 255. / total[3]).round() as u8;
                }
                pixels[offset + 3] = alpha.round() as u8;
            }
        }
    }
    pixels
}

pub fn dib(size: u32) -> Vec<u8> {
    let rgba = rgba(size);
    let mask_stride = size.div_ceil(32) * 4;
    let mut dib = vec![0u8; (40 + size * size * 4 + mask_stride * size) as usize];
    dib[0..4].copy_from_slice(&40u32.to_le_bytes());
    dib[4..8].copy_from_slice(&(size as i32).to_le_bytes());
    dib[8..12].copy_from_slice(&((size * 2) as i32).to_le_bytes());
    dib[12..14].copy_from_slice(&1u16.to_le_bytes());
    dib[14..16].copy_from_slice(&32u16.to_le_bytes());
    dib[20..24].copy_from_slice(&(size * size * 4).to_le_bytes());
    for y in 0..size {
        for x in 0..size {
            let from = ((y * size + x) * 4) as usize;
            let to = (40 + ((size - 1 - y) * size + x) * 4) as usize;
            dib[to..to + 4].copy_from_slice(&[
                rgba[from + 2],
                rgba[from + 1],
                rgba[from],
                rgba[from + 3],
            ]);
            if rgba[from + 3] == 0 {
                let mask = (40 + size * size * 4 + (size - 1 - y) * mask_stride + x / 8) as usize;
                dib[mask] |= 0x80 >> (x % 8);
            }
        }
    }
    dib
}
