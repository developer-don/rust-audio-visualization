use anyhow::Result;
use glam::{Mat4, Vec3A};
use std::sync::Arc;
use wgpu::util::DeviceExt;

const SHADERS_WGSL: &str = r#"
struct VertexInput {
    @location(0) position: vec3<f32>,
};

struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
};

@group(0) @binding(0)
var<uniform> mvp: mat4x4<f32>;

@vertex
fn vs_main(model: VertexInput) -> VertexOutput {
    var out: VertexOutput;
    out.clip_position = mvp * vec4<f32>(model.position, 1.0);
    return out;
}

@fragment
fn fs_main() -> @location(0) vec4<f32> {
    return vec4<f32>(0.8, 0.8, 0.8, 1.0); // Opaque light gray points
}
"#;

// TODO: Remove `mvp_bind_group_layout` if we're only going to pass it around.
#[allow(dead_code)]
pub struct SphereWgpuPrimitive {
    vertex_buffer: wgpu::Buffer,
    num_vertices: u32,
    mvp_bind_group_layout: wgpu::BindGroupLayout,
    mvp_uniform_buffer: wgpu::Buffer,
    mvp_bind_group: wgpu::BindGroup,
    render_pipeline: wgpu::RenderPipeline,
}

pub struct WgpuSphereRenderer {
    primitive: Option<Arc<SphereWgpuPrimitive>>,
    points: Vec<[f32; 3]>,
    camera_position: Vec3A,
    pub time: f32,
}

impl WgpuSphereRenderer {
    pub fn new(points_data: Vec<[f32; 3]>) -> Self {
        Self {
            primitive: None,
            points: points_data,
            camera_position: Vec3A::new(0.0, 0.0, 3.0),
            time: 0.0,
        }
    }

    // TODO: Remove `queue` from as an argument if we don't end up needing it.
    pub fn prepare(
        &mut self,
        device: &Arc<wgpu::Device>,
        _queue: &Arc<wgpu::Queue>,
        target_format: wgpu::TextureFormat,
    ) -> Result<()> {
        if self.primitive.is_some() {
            return Ok(());
        }

        tracing::info!("Preparing WgpuSphereRenderer resources...");

        let shader_module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Sphere Shader"),
            source: wgpu::ShaderSource::Wgsl(SHADERS_WGSL.into()),
        });

        let vertex_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Sphere Vertex Buffer"),
            contents: bytemuck::cast_slice(&self.points),
            usage: wgpu::BufferUsages::VERTEX,
        });
        let num_vertices = self.points.len() as u32;

        let mvp_matrix_initial = Mat4::IDENTITY;
        let mvp_uniform_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Sphere MVP Uniform Buffer"),
            contents: bytemuck::cast_slice(&[mvp_matrix_initial]),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });

        let mvp_bind_group_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("Sphere MVP Bind Group Layout"),
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

        let mvp_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Sphere MVP Bind Group"),
            layout: &mvp_bind_group_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: mvp_uniform_buffer.as_entire_binding(),
            }],
        });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Sphere Render Pipeline Layout"),
            bind_group_layouts: &[&mvp_bind_group_layout],
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
            mvp_bind_group_layout,
            mvp_bind_group,
            render_pipeline,
        }));

        tracing::info!("WgpuSphereRenderer resources prepared successfully.");

        Ok(())
    }

    pub fn calculate_mvp(&self, aspect_ratio: f32) -> Mat4 {
        let view = Mat4::look_at_rh(
            self.camera_position.into(),
            Vec3A::ZERO.into(),
            Vec3A::Y.into(),
        );
        let model =
            Mat4::from_rotation_y(self.time * 0.4) * Mat4::from_rotation_x(self.time * 0.25);
        let proj = Mat4::perspective_rh_gl(std::f32::consts::FRAC_PI_4, aspect_ratio, 0.1, 100.0);

        proj * view * model
    }

    pub fn get_primitive_arc(&self) -> Option<Arc<SphereWgpuPrimitive>> {
        self.primitive.clone()
    }

    pub fn paint_primitive<'rp_lifetime>(
        primitive: &'rp_lifetime SphereWgpuPrimitive, // Tie primitive's borrow to rp_lifetime
        mvp_matrix: &Mat4, // mvp_matrix can have its own independent lifetime
        rpass: &mut wgpu::RenderPass<'rp_lifetime>, // RenderPass needs to use rp_lifetime
        queue: &Arc<wgpu::Queue>,
    ) {
        queue.write_buffer(
            &primitive.mvp_uniform_buffer,
            0,
            bytemuck::cast_slice(&[*mvp_matrix]),
        );

        rpass.set_pipeline(&primitive.render_pipeline);
        rpass.set_bind_group(0, &primitive.mvp_bind_group, &[]);
        rpass.set_vertex_buffer(0, primitive.vertex_buffer.slice(..));
        rpass.draw(0..primitive.num_vertices, 0..1);
    }
}
