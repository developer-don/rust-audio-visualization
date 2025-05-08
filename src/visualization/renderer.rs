use crate::audio::{AudioAnalysisData, PlaybackState};
use anyhow::Result;
use bytemuck::{Pod, Zeroable};
use glam::{Mat4, Vec3A};
use std::sync::Arc;
use wgpu::util::DeviceExt;

// Uniform struct - Only color
#[repr(C)]
#[derive(Debug, Copy, Clone, Pod, Zeroable)]
struct VisualParamsUniform {
    // Rust struct name can remain different if desired
    color: [f32; 4],
}

// Shader source (WGSL) - Corrected struct usage
const SHADERS_WGSL: &str = r#"
// === UNIFORMS ===
@group(0) @binding(0)
var<uniform> mvp: mat4x4<f32>;

// Group 1: Visual parameters (Color only)
struct VisualParams { // Define struct as VisualParams
    color: vec4<f32>,
};
@group(1) @binding(0)
var<uniform> visual_params: VisualParams; // Use the defined struct name VisualParams

// === VERTEX SHADER ===
struct VertexInput {
    @location(0) position: vec3<f32>,
};

struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
};

@vertex
fn vs_main(model: VertexInput) -> VertexOutput {
    var out: VertexOutput;
    out.clip_position = mvp * vec4<f32>(model.position, 1.0);
    return out;
}

// === FRAGMENT SHADER ===
@fragment
fn fs_main() -> @location(0) vec4<f32> {
    return visual_params.color; // Access color field of the VisualParams uniform
}
"#;

pub struct SphereWgpuPrimitive {
    vertex_buffer: wgpu::Buffer,
    num_vertices: u32,
    mvp_uniform_buffer: wgpu::Buffer,
    mvp_bind_group: wgpu::BindGroup,
    visual_params_uniform_buffer: wgpu::Buffer, // For color
    visual_params_bind_group: wgpu::BindGroup,  // For color
    render_pipeline: wgpu::RenderPipeline,
}

pub struct WgpuSphereRenderer {
    primitive: Option<Arc<SphereWgpuPrimitive>>,
    points: Vec<[f32; 3]>,
    camera_position: Vec3A,
    pub time: f32,
    // Visual state based on audio
    current_scale: f32, // Re-added for overall sphere size animation
    current_hue: f32,
    current_saturation: f32,
    current_value: f32,
    pub current_color_rgb: [f32; 3],
}

impl WgpuSphereRenderer {
    pub fn new(points_data: Vec<[f32; 3]>) -> Self {
        Self {
            primitive: None,
            points: points_data,
            camera_position: Vec3A::new(0.0, 0.0, 4.0), // User's camera position
            time: 0.0,
            current_scale: 1.25, // User's initial scale
            // Initialize color state
            current_hue: 0.0,
            current_saturation: 0.5,
            current_value: 1.0,
            current_color_rgb: hsv_to_rgb(0.0, 0.5, 1.0),
        }
    }

    pub fn prepare(
        &mut self,
        device: &Arc<wgpu::Device>,
        target_format: wgpu::TextureFormat,
    ) -> Result<()> {
        if self.primitive.is_some() {
            return Ok(());
        }
        tracing::info!("Preparing WgpuSphereRenderer resources (Color Uniform)...");

        let shader_module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Sphere Shader"),
            source: wgpu::ShaderSource::Wgsl(SHADERS_WGSL.into()),
        });

        // --- MVP Resources (Group 0) ---
        let mvp_bind_group_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("MVP Bind Group Layout"),
                entries: &[wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                }],
            });
        let mvp_matrix_initial = Mat4::IDENTITY;
        let mvp_uniform_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Sphere MVP Uniform Buffer"),
            contents: bytemuck::cast_slice(&[mvp_matrix_initial]),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });
        let mvp_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Sphere MVP Bind Group"),
            layout: &mvp_bind_group_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: mvp_uniform_buffer.as_entire_binding(),
            }],
        });

        // --- Visual Params Resources (Group 1 - Color only) ---
        let visual_params_bind_group_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("Visual Params Bind Group Layout"),
                entries: &[wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: Some(
                            std::num::NonZeroU64::new(
                                std::mem::size_of::<VisualParamsUniform>() as u64
                            )
                            .unwrap(),
                        ),
                    },
                    count: None,
                }],
            });
        let visual_params_initial = VisualParamsUniform {
            color: [
                self.current_color_rgb[0],
                self.current_color_rgb[1],
                self.current_color_rgb[2],
                1.0,
            ],
        };
        let visual_params_uniform_buffer =
            device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("Visual Params Uniform Buffer"),
                contents: bytemuck::bytes_of(&visual_params_initial),
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            });
        let visual_params_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Visual Params Bind Group"),
            layout: &visual_params_bind_group_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: visual_params_uniform_buffer.as_entire_binding(),
            }],
        });

        // --- Vertex Buffer ---
        let vertex_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Sphere Vertex Buffer"),
            contents: bytemuck::cast_slice(&self.points),
            usage: wgpu::BufferUsages::VERTEX,
        });
        let num_vertices = self.points.len() as u32;

        // --- Pipeline ---
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Sphere Render Pipeline Layout"),
            bind_group_layouts: &[&mvp_bind_group_layout, &visual_params_bind_group_layout],
            push_constant_ranges: &[],
        });
        let render_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("Sphere Render Pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader_module,
                entry_point: "vs_main",
                buffers: &[wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<[f32; 3]>() as wgpu::BufferAddress,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &wgpu::vertex_attr_array![0 => Float32x3],
                }],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader_module,
                entry_point: "fs_main",
                targets: &[Some(wgpu::ColorTargetState {
                    format: target_format,
                    blend: Some(wgpu::BlendState::REPLACE),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::PointList,
                strip_index_format: None,
                front_face: wgpu::FrontFace::Ccw,
                cull_mode: None,
                polygon_mode: wgpu::PolygonMode::Fill,
                unclipped_depth: false,
                conservative: false,
            },
            depth_stencil: None,
            multisample: wgpu::MultisampleState {
                count: 1,
                mask: !0,
                alpha_to_coverage_enabled: false,
            },
            multiview: None,
        });

        self.primitive = Some(Arc::new(SphereWgpuPrimitive {
            vertex_buffer,
            num_vertices,
            mvp_uniform_buffer,
            mvp_bind_group,
            visual_params_uniform_buffer,
            visual_params_bind_group,
            render_pipeline,
        }));
        tracing::info!("WgpuSphereRenderer resources prepared successfully.");
        Ok(())
    }

    /// Update visual state (color, scale) based on audio playback state and analysis data.
    pub fn update_visual_state(
        &mut self,
        playback_state: PlaybackState,
        audio_data: &Option<AudioAnalysisData>,
    ) {
        self.current_hue = (self.time * 0.05).fract(); // Hue cycles based on time

        let target_saturation;
        let target_scale; // Target for overall sphere scale

        if playback_state == PlaybackState::Playing {
            if let Some(data) = audio_data {
                let amplitude_factor = (data.rms_amplitude * 2.0).clamp(0.0, 1.0); // Normalize RMS somewhat
                                                                                   // Saturation increases with amplitude
                target_saturation = 0.5 + amplitude_factor * 0.5;
                // Scale increases with amplitude (using user's previous logic)
                target_scale = 0.75 + (amplitude_factor * 2.5); // Map amplitude to scale (0.75 to 3.25 approx)
            } else {
                // Playing but no data (silence)
                target_saturation = 0.5;
                target_scale = 0.75; // Minimum scale when silent but playing
            }
        } else {
            // Idle, Paused, Loaded
            target_saturation = 0.5; // Idle saturation
            target_scale = 1.25; // User's default idle scale
        }

        // Smooth towards target values
        let lerp_factor = 0.1;
        self.current_saturation += (target_saturation - self.current_saturation) * lerp_factor;
        self.current_scale += (target_scale - self.current_scale) * lerp_factor;

        // Clamp scale (using user's previous clamping)
        self.current_scale = self.current_scale.clamp(0.75, 7.50);

        // Convert final HSV to RGB
        self.current_color_rgb = hsv_to_rgb(
            self.current_hue,
            self.current_saturation,
            self.current_value,
        );
    }

    // Calculate MVP matrix, re-applying the overall scale
    pub fn calculate_mvp(&self, aspect_ratio: f32) -> Mat4 {
        let view = Mat4::look_at_rh(
            self.camera_position.into(),
            Vec3A::ZERO.into(),
            Vec3A::Y.into(),
        );
        // Apply rotation AND scale from self.current_scale
        let model = Mat4::from_rotation_y(self.time * 0.4)
            * Mat4::from_rotation_x(self.time * 0.25)
            * Mat4::from_scale(Vec3A::splat(self.current_scale).into()); // Re-added scale

        let proj = Mat4::perspective_rh_gl(std::f32::consts::FRAC_PI_4, aspect_ratio, 0.1, 100.0);
        proj * view * model
    }

    pub fn get_primitive_arc(&self) -> Option<Arc<SphereWgpuPrimitive>> {
        self.primitive.clone()
    }

    // Paint primitive - updates color uniform only
    pub fn paint_primitive<'rp_lifetime>(
        primitive: &'rp_lifetime SphereWgpuPrimitive,
        mvp_matrix: &Mat4,
        color: &[f32; 3], // Pass color calculated in update_visual_state
        // point_size removed
        rpass: &mut wgpu::RenderPass<'rp_lifetime>,
        queue: &Arc<wgpu::Queue>,
    ) {
        queue.write_buffer(
            &primitive.mvp_uniform_buffer,
            0,
            bytemuck::cast_slice(&[*mvp_matrix]),
        );

        let visual_data = VisualParamsUniform {
            color: [color[0], color[1], color[2], 1.0],
        };
        queue.write_buffer(
            &primitive.visual_params_uniform_buffer,
            0,
            bytemuck::bytes_of(&visual_data),
        );

        rpass.set_pipeline(&primitive.render_pipeline);
        rpass.set_bind_group(0, &primitive.mvp_bind_group, &[]);
        rpass.set_bind_group(1, &primitive.visual_params_bind_group, &[]); // Color is group 1
        rpass.set_vertex_buffer(0, primitive.vertex_buffer.slice(..));
        rpass.draw(0..primitive.num_vertices, 0..1);
    }
}

fn hsv_to_rgb(h: f32, s: f32, v: f32) -> [f32; 3] {
    if s <= 0.0 {
        return [v, v, v];
    }
    let h_scaled = h * 6.0;
    let sector = h_scaled.floor();
    let f = h_scaled - sector;
    let p = v * (1.0 - s);
    let q = v * (1.0 - s * f);
    let t = v * (1.0 - s * (1.0 - f));
    match sector as i32 % 6 {
        0 => [v, t, p],
        1 => [q, v, p],
        2 => [p, v, t],
        3 => [p, q, v],
        4 => [t, p, v],
        5 => [v, p, q],
        _ => unreachable!(),
    }
}
