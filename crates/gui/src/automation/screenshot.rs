//! Render the wizard into a texture without creating a native window.
use super::{App, Screenshot, egui, wizard_frame};
use egui_wgpu::{Renderer, RendererOptions, ScreenDescriptor, wgpu};
use image::ImageEncoder as _;
use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

const WIDTH: u32 = 1760;
const HEIGHT: u32 = 1240;
const SCALE: f32 = 2.;

pub(super) fn capture(app: &mut App, path: PathBuf) -> Result<Screenshot, String> {
    let publish_error = |err| format!("Could not write screenshot {}: {err}", path.display());
    match path.symlink_metadata() {
        Ok(_) => {
            return Err(format!(
                "Screenshot path already exists: {}",
                path.display()
            ));
        }
        Err(err) if err.kind() == io::ErrorKind::NotFound => {}
        Err(err) => return Err(publish_error(err)),
    }
    let parent = path
        .parent()
        .filter(|dir| !dir.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let mut staged = tempfile::NamedTempFile::new_in(parent).map_err(publish_error)?;
    let pixels = render(app)?;
    image::codecs::png::PngEncoder::new(staged.as_file_mut())
        .write_image(&pixels, WIDTH, HEIGHT, image::ExtendedColorType::Rgba8)
        .map_err(|err| format!("Could not encode screenshot: {err}"))?;
    staged
        .persist_noclobber(&path)
        .map_err(|err| publish_error(err.error))?;
    Ok(Screenshot {
        path,
        width: WIDTH,
        height: HEIGHT,
    })
}

fn render(app: &mut App) -> Result<Vec<u8>, String> {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
        backends: wgpu::Backends::METAL,
        ..wgpu::InstanceDescriptor::new_without_display_handle()
    });
    let adapter = pollster::block_on(instance.request_adapter(&Default::default()))
        .map_err(|err| format!("Could not open the offscreen graphics adapter: {err}"))?;
    let (device, queue) = pollster::block_on(adapter.request_device(&Default::default()))
        .map_err(|err| format!("Could not initialize offscreen graphics: {err}"))?;

    let format = wgpu::TextureFormat::Rgba8Unorm;
    let size = wgpu::Extent3d {
        width: WIDTH,
        height: HEIGHT,
        depth_or_array_layers: 1,
    };
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("wizard screenshot"),
        size,
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let view = texture.create_view(&Default::default());
    let mut renderer = Renderer::new(&device, format, RendererOptions::PREDICTABLE);
    let ctx = egui::Context::default();
    App::theme(&ctx);
    let screen = egui::Rect::from_min_size(
        egui::Pos2::ZERO,
        egui::vec2(WIDTH as f32 / SCALE, HEIGHT as f32 / SCALE),
    );
    let mut output = wizard_frame(app, &ctx, screen, SCALE);
    let jobs = ctx.tessellate(std::mem::take(&mut output.shapes), output.pixels_per_point);
    let screen = ScreenDescriptor {
        size_in_pixels: [WIDTH, HEIGHT],
        pixels_per_point: output.pixels_per_point,
    };
    for (id, deltas) in &output.textures_delta.set {
        for delta in deltas {
            renderer.update_texture(&device, &queue, *id, delta);
        }
    }
    // This renderer is discarded after this frame, including any textures awaiting free.
    output.drop_without_applying_deltas();

    let mut encoder = device.create_command_encoder(&Default::default());
    let uploads = renderer.update_buffers(&device, &queue, &mut encoder, &jobs, &screen);
    renderer.render(
        &mut encoder
            .begin_render_pass(&wgpu::RenderPassDescriptor {
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
            })
            .forget_lifetime(),
        &jobs,
        &screen,
    );

    let row_bytes = WIDTH * 4;
    let stride = row_bytes.next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("wizard screenshot readback"),
        size: u64::from(stride) * u64::from(HEIGHT),
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    encoder.copy_texture_to_buffer(
        texture.as_image_copy(),
        wgpu::TexelCopyBufferInfo {
            buffer: &buffer,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(stride),
                rows_per_image: None,
            },
        },
        size,
    );
    let submission = queue.submit(uploads.into_iter().chain(std::iter::once(encoder.finish())));
    let slice = buffer.slice(..);
    let (tx, rx) = std::sync::mpsc::sync_channel(1);
    slice.map_async(wgpu::MapMode::Read, move |result| {
        let _ = tx.send(result);
    });
    device
        .poll(wgpu::PollType::Wait {
            submission_index: Some(submission),
            timeout: Some(Duration::from_secs(10)),
        })
        .map_err(|err| format!("Offscreen render did not complete: {err}"))?;
    rx.try_recv()
        .map_err(|err| format!("No screenshot readback result: {err}"))?
        .map_err(|err| format!("Could not read screenshot pixels: {err}"))?;
    let mapped = slice
        .get_mapped_range()
        .map_err(|err| format!("Could not access screenshot pixels: {err}"))?;
    let mut pixels = Vec::with_capacity((row_bytes * HEIGHT) as usize);
    for row in mapped.chunks_exact(stride as usize) {
        pixels.extend_from_slice(&row[..row_bytes as usize]);
    }
    drop(mapped);
    buffer.unmap();
    Ok(pixels)
}
