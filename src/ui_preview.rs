//! Test-only software preview of the real egui meshes; no native window is opened.
use eframe::egui::{self};
use std::collections::HashMap;
#[derive(Default)]
pub struct Renderer {
    textures: HashMap<egui::TextureId, egui::ColorImage>,
}
impl Renderer {
    pub fn save(
        &mut self,
        ctx: &egui::Context,
        mut out: egui::FullOutput,
        size: [usize; 2],
        path: &std::path::Path,
    ) {
        for (id, deltas) in &out.textures_delta.set {
            for delta in deltas {
                let egui::ImageData::Color(image) = &delta.image;
                if let Some([x, y]) = delta.pos {
                    let target = self
                        .textures
                        .get_mut(id)
                        .expect("texture patch without base");
                    for row in 0..image.size[1] {
                        let start = (y + row) * target.size[0] + x;
                        target.pixels[start..start + image.size[0]].copy_from_slice(
                            &image.pixels[row * image.size[0]..(row + 1) * image.size[0]],
                        );
                    }
                } else {
                    self.textures.insert(*id, (**image).clone());
                }
            }
        }
        let free = out.textures_delta.free.clone();
        out.textures_delta.clear();
        let meshes = ctx.tessellate(out.shapes, out.pixels_per_point);
        let mut pixels = vec![[18u8, 22, 29, 255]; size[0] * size[1]];
        for primitive in meshes {
            let egui::epaint::Primitive::Mesh(mesh) = primitive.primitive else {
                continue;
            };
            let texture = self
                .textures
                .get(&mesh.texture_id)
                .expect("missing mesh texture");
            for tri in mesh.indices.chunks_exact(3) {
                let v = [
                    mesh.vertices[tri[0] as usize],
                    mesh.vertices[tri[1] as usize],
                    mesh.vertices[tri[2] as usize],
                ];
                let cross = |a: egui::Pos2, b: egui::Pos2, p: egui::Pos2| {
                    (b.x - a.x) * (p.y - a.y) - (b.y - a.y) * (p.x - a.x)
                };
                let area = cross(v[0].pos, v[1].pos, v[2].pos);
                if area.abs() < 0.0001 {
                    continue;
                }
                let bounds = egui::Rect::from_min_max(
                    egui::pos2(
                        v.iter().map(|v| v.pos.x).fold(f32::INFINITY, f32::min),
                        v.iter().map(|v| v.pos.y).fold(f32::INFINITY, f32::min),
                    ),
                    egui::pos2(
                        v.iter().map(|v| v.pos.x).fold(f32::NEG_INFINITY, f32::max),
                        v.iter().map(|v| v.pos.y).fold(f32::NEG_INFINITY, f32::max),
                    ),
                )
                .intersect(primitive.clip_rect)
                .intersect(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(size[0] as f32, size[1] as f32),
                ));
                if !bounds.is_positive() {
                    continue;
                }
                for y in bounds.min.y.floor().max(0.) as usize
                    ..(bounds.max.y.ceil() as usize).min(size[1])
                {
                    for x in bounds.min.x.floor().max(0.) as usize
                        ..(bounds.max.x.ceil() as usize).min(size[0])
                    {
                        let p = egui::pos2(x as f32 + 0.5, y as f32 + 0.5);
                        if !primitive.clip_rect.contains(p) {
                            continue;
                        }
                        let w = [
                            cross(v[1].pos, v[2].pos, p) / area,
                            cross(v[2].pos, v[0].pos, p) / area,
                            cross(v[0].pos, v[1].pos, p) / area,
                        ];
                        // Strict edge rule avoids blending a shared edge twice.
                        if w.iter().any(|w| *w < 0.) {
                            continue;
                        }
                        let uv = v[0].uv.to_vec2() * w[0]
                            + v[1].uv.to_vec2() * w[1]
                            + v[2].uv.to_vec2() * w[2];
                        let tx = (uv.x * texture.size[0] as f32 - 0.5)
                            .clamp(0., texture.size[0] as f32 - 1.);
                        let ty = (uv.y * texture.size[1] as f32 - 0.5)
                            .clamp(0., texture.size[1] as f32 - 1.);
                        let ix = tx.floor() as usize;
                        let iy = ty.floor() as usize;
                        let fx = tx.fract();
                        let fy = ty.fract();
                        let texels = [
                            texture.pixels[iy * texture.size[0] + ix],
                            texture.pixels
                                [iy * texture.size[0] + (ix + 1).min(texture.size[0] - 1)],
                            texture.pixels
                                [(iy + 1).min(texture.size[1] - 1) * texture.size[0] + ix],
                            texture.pixels[(iy + 1).min(texture.size[1] - 1) * texture.size[0]
                                + (ix + 1).min(texture.size[0] - 1)],
                        ];
                        let tw = [
                            (1. - fx) * (1. - fy),
                            fx * (1. - fy),
                            (1. - fx) * fy,
                            fx * fy,
                        ];
                        let mut src = [0.; 4];
                        for c in 0..4 {
                            let vc = (0..3).map(|i| v[i].color[c] as f32 * w[i]).sum::<f32>();
                            let tc = (0..4).map(|i| texels[i][c] as f32 * tw[i]).sum::<f32>();
                            src[c] = vc * tc / 255.;
                        }
                        let dst = &mut pixels[y * size[0] + x];
                        for c in 0..3 {
                            dst[c] = (src[c] + dst[c] as f32 * (1. - src[3] / 255.)).clamp(0., 255.)
                                as u8;
                        }
                    }
                }
            }
        }
        let bytes: Vec<u8> = pixels.into_iter().flatten().collect();
        std::fs::write(path, bytes).unwrap();
        for id in free {
            self.textures.remove(&id);
        }
    }
}
