//! Desktop rendering policy. Keep driver-specific choices out of the search and row modules.
use clap::ValueEnum;

#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
pub enum RendererChoice {
    /// Windows Direct3D 12 output, independent of the OpenGL driver path.
    Direct3d,
    /// Compatibility/comparison output using the previous OpenGL renderer.
    Opengl,
}
impl Default for RendererChoice {
    fn default() -> Self {
        if cfg!(windows) {
            Self::Direct3d
        } else {
            Self::Opengl
        }
    }
}
pub fn options(choice: RendererChoice) -> eframe::NativeOptions {
    let mut options = eframe::NativeOptions::default();
    match choice {
        RendererChoice::Direct3d => {
            let mut setup = eframe::egui_wgpu::WgpuSetupCreateNew::without_display_handle();
            setup.instance_descriptor.backends = wgpu::Backends::DX12;
            options.renderer = eframe::Renderer::Wgpu;
            options.wgpu_options.wgpu_setup = setup.into();
            options.wgpu_options.surface = eframe::SurfaceConfig::LOW_LATENCY;
        }
        RendererChoice::Opengl => options.renderer = eframe::Renderer::Glow,
    }
    options
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn desktop_renderer_choice_keeps_an_explicit_compatibility_path() {
        assert_eq!(
            RendererChoice::from_str("direct3d", false).unwrap(),
            RendererChoice::Direct3d
        );
        assert_eq!(
            RendererChoice::from_str("opengl", false).unwrap(),
            RendererChoice::Opengl
        );
        assert!(RendererChoice::from_str("invalid", false).is_err());
        assert_eq!(
            options(RendererChoice::Opengl).renderer,
            eframe::Renderer::Glow
        );
        let direct = options(RendererChoice::Direct3d);
        assert_eq!(direct.renderer, eframe::Renderer::Wgpu);
        let eframe::egui_wgpu::WgpuSetup::CreateNew(setup) = direct.wgpu_options.wgpu_setup else {
            panic!("expected owned device setup")
        };
        assert_eq!(setup.instance_descriptor.backends, wgpu::Backends::DX12);
        assert_eq!(
            direct.wgpu_options.surface,
            eframe::SurfaceConfig::LOW_LATENCY
        );
    }
    #[test]
    #[ignore = "uses the real Direct3D adapter/device without opening a window"]
    fn real_direct3d_device_and_egui_pipeline() {
        pollster::block_on(async {
            let config = options(RendererChoice::Direct3d).wgpu_options;
            let start = std::time::Instant::now();
            let instance = config.wgpu_setup.new_instance().await;
            let state = eframe::egui_wgpu::RenderState::create(
                &config,
                &instance,
                None,
                Default::default(),
            )
            .await
            .unwrap();
            let info = state.adapter.get_info();
            assert_eq!(info.backend, wgpu::Backend::Dx12);
            let texture = state.device.create_texture(&wgpu::TextureDescriptor {
                label: Some("REA headless Direct3D verification"),
                size: wgpu::Extent3d {
                    width: 64,
                    height: 64,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: state.target_format,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                view_formats: &[],
            });
            let view = texture.create_view(&Default::default());
            let mut encoder = state.device.create_command_encoder(&Default::default());
            {
                let _pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("REA Direct3D offscreen pass"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &view,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                            store: wgpu::StoreOp::Store,
                        },
                        depth_slice: None,
                    })],
                    ..Default::default()
                });
            }
            let submission = state.queue.submit([encoder.finish()]);
            state
                .device
                .poll(wgpu::PollType::Wait {
                    submission_index: Some(submission),
                    timeout: Some(std::time::Duration::from_secs(10)),
                })
                .unwrap();
            println!(
                "{}",
                serde_json::json!({"backend":format!("{:?}",info.backend),"adapter":info.name,"device_type":format!("{:?}",info.device_type),"initialization_and_offscreen_ms":start.elapsed().as_secs_f64()*1000.,"scope":"real D3D device + egui pipeline creation + offscreen submission; no desktop presentation"})
            );
        });
    }
}
