use glyphon::cosmic_text::skrifa::instance;
use glyphon::{
    Attrs, Buffer, Cache, Color, Family, Font, FontSystem, Metrics, Resolution, Shaping,
    SwashCache, TextArea, TextAtlas, TextBounds, TextRenderer, Viewport,
};
use std::sync::Arc;
use wgpu::util::DeviceExt;
use wgpu::{
    CommandEncoderDescriptor, CompositeAlphaMode, DeviceDescriptor, Instance, InstanceDescriptor,
    LoadOp, MultisampleState, Operations, PresentMode, RenderPassColorAttachment,
    RenderPassDescriptor, RequestAdapterOptions, SurfaceConfiguration, TextureFormat,
    TextureUsages, TextureViewDescriptor,
};
use winit::{
    application::ApplicationHandler,
    event::WindowEvent,
    event_loop::{ActiveEventLoop, EventLoop},
    window::Window,
};

#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
struct RectInstance {
    position: [f32; 2],
    size: [f32; 2],
    color: [f32; 4],
}

#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
struct Globals {
    resolution: [f32; 2],
    _pad: [f32; 2],
}

fn build_rects(w: f32, h: f32) -> Vec<RectInstance> {
    let rect_height = (h - 60.0) / 2.0;
    let mut rects = vec![
        // query bar
        RectInstance {
            position: [20.0, 20.0],
            size: [w - 40.0, rect_height],
            color: [0.15, 0.15, 0.15, 1.0],
        },
        // results panel background
        RectInstance {
            position: [20.0, 30.0 + rect_height],
            size: [w - 40.0, rect_height],
            color: [0.1, 0.1, 0.1, 1.0],
        },
    ];

    let table_x = 20.0;
    let table_y = 30.0 + rect_height;
    let table_w = w - 40.0;
    let cols = 3;
    let rows = 5;
    let col_width = table_w / cols as f32;
    let row_height = 30.0;

    for r in 0..rows {
        for c in 0..cols {
            rects.push(RectInstance {
                position: [
                    table_x + c as f32 * col_width,
                    table_y + r as f32 * row_height,
                ],
                size: [col_width - 2.0, row_height - 2.0], // -2 for gap between cells
                color: if r == 0 {
                    [0.2, 0.2, 0.2, 1.0]
                } else {
                    [0.12, 0.12, 0.12, 1.0]
                },
            });
        }
    }

    rects
}

#[derive(Default)]
pub struct App {
    state: Option<State>,
}
pub struct State {
    surface: wgpu::Surface<'static>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    config: wgpu::SurfaceConfiguration,

    // Text glyphon stuff
    font_system: FontSystem,
    swash_cache: SwashCache,
    viewport: Viewport,
    atlas: glyphon::TextAtlas,
    text_renderer: glyphon::TextRenderer,
    text_buffer: glyphon::Buffer,
    table_buffers: Vec<glyphon::Buffer>,

    // Shaders stuff
    globals_buffer: wgpu::Buffer,
    globals_bind_group: wgpu::BindGroup,
    rect_buffer: wgpu::Buffer,
    rect_pipeline: wgpu::RenderPipeline,

    // UI stuff
    query: String,

    // window has to be last because a bug or something
    window: Arc<Window>,
}

impl State {
    pub async fn new(window: Arc<Window>) -> anyhow::Result<Self> {
        let instance = wgpu::Instance::default();
        let surface = instance.create_surface(window.clone())?;

        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                compatible_surface: Some(&surface),
                ..Default::default()
            })
            .await?;

        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor::default())
            .await?;

        let size = window.inner_size();
        let config = surface
            .get_default_config(&adapter, size.width, size.height)
            .ok_or(anyhow::anyhow!("no surface config"))?;

        surface.configure(&device, &config);

        // TExt glyphon stuff
        let mut font_system = FontSystem::new();
        let swash_cache = SwashCache::new();
        let cache = Cache::new(&device);
        let viewport = Viewport::new(&device, &cache);
        let mut atlas = TextAtlas::new(&device, &queue, &cache, config.format);
        let text_renderer =
            TextRenderer::new(&mut atlas, &device, MultisampleState::default(), None);

        let mut text_buffer = Buffer::new(&mut font_system, Metrics::new(30.0, 42.0));
        text_buffer.set_size(
            &mut font_system,
            Some(size.width as f32),
            Some(size.height as f32),
        );

        let query = String::from("SELECT *, magnus FROM kaiser;");

        text_buffer.set_text(
            &mut font_system,
            &query,
            &Attrs::new().family(Family::SansSerif),
            Shaping::Advanced,
            None,
        );
        text_buffer.shape_until_scroll(&mut font_system, false);

        let fake_data = vec![
            vec!["id", "name", "email"],
            vec!["1", "alice", "alice@example.com"],
            vec!["2", "bob", "bob@example.com"],
            vec!["3", "carol", "carol@example.com"],
            vec!["4", "dave", "dave@example.com"],
        ];

        let mut table_buffers = vec![];
        for row in &fake_data {
            for cell in row {
                let mut buf = Buffer::new(&mut font_system, Metrics::new(16.0, 20.0));
                buf.set_size(&mut font_system, Some(200.0), Some(30.0));
                buf.set_text(
                    &mut font_system,
                    cell,
                    &Attrs::new().family(Family::Monospace),
                    Shaping::Advanced,
                    None,
                );
                buf.shape_until_scroll(&mut font_system, false);
                table_buffers.push(buf);
            }
        }
        // Shader stuff
        //  // Globals uniform
        let globals = Globals {
            resolution: [size.width as f32, size.height as f32],
            _pad: [0.0; 2],
        };
        let globals_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("globals"),
            contents: bytemuck::cast_slice(&[globals]),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });
        let globals_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: None,
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

        let globals_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: &globals_bgl,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: globals_buffer.as_entire_binding(),
            }],
        });

        // Rect buffer
        let rects = build_rects(size.width as f32, size.height as f32);
        let rect_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("rects"),
            size: (std::mem::size_of::<RectInstance>() * 1024) as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        queue.write_buffer(
            &rect_buffer,
            0,
            bytemuck::cast_slice(&build_rects(size.width as f32, size.height as f32)),
        );
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shaders.wgsl").into()),
        });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: None,
            bind_group_layouts: &[&globals_bgl],
            immediate_size: 0,
        });

        let rect_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: None,
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                buffers: &[wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<RectInstance>() as u64,
                    step_mode: wgpu::VertexStepMode::Instance,
                    attributes: &wgpu::vertex_attr_array![
                        0 => Float32x2,
                        1 => Float32x2,
                        2 => Float32x4,
                    ],
                }],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                targets: &[Some(wgpu::ColorTargetState {
                    format: config.format,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: Default::default(),
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multiview_mask: None,
            multisample: wgpu::MultisampleState::default(),
            cache: None,
        });

        Ok(Self {
            surface,
            device,
            queue,
            config,

            font_system,
            swash_cache,
            viewport,
            atlas,
            text_renderer,
            text_buffer,
            table_buffers,

            // Shader stuff
            rect_pipeline,
            globals_buffer,
            globals_bind_group,
            rect_buffer,

            // UI stuff
            query,

            window,
        })
    }

    pub fn resize(&mut self, width: u32, height: u32) {
        if width > 0 && height > 0 {
            self.config.width = width;
            self.config.height = height;
            self.surface.configure(&self.device, &self.config);
            self.text_buffer.set_size(
                &mut self.font_system,
                Some(width as f32),
                Some(height as f32),
            );
            self.text_buffer
                .shape_until_scroll(&mut self.font_system, false);
        }
    }

    pub fn render(&mut self) {
        let frame = match self.surface.get_current_texture() {
            Ok(frame) => frame,
            Err(wgpu::SurfaceError::Outdated) | Err(wgpu::SurfaceError::Lost) => {
                self.surface.configure(&self.device, &self.config);
                self.window.request_redraw();
                return;
            }
            Err(e) => panic!("Surface error: {e}"),
        };

        let view = frame
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());

        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor::default());

        self.viewport.update(
            &self.queue,
            Resolution {
                width: self.config.width,
                height: self.config.height,
            },
        );
        let rect_height = (self.config.height as f32 - 60.0) / 2.0;
        let table_x = 20.0_f32;
        let table_y = 30.0 + rect_height;
        let table_w = self.config.width as f32 - 40.0;
        let cols = 3_usize;
        let row_height = 30.0_f32;
        let col_width = table_w / cols as f32;

        let mut text_areas: Vec<TextArea> = vec![
            // query input
            TextArea {
                buffer: &self.text_buffer,
                left: 28.0,
                top: 28.0,
                scale: 1.0,
                bounds: TextBounds {
                    left: 20,
                    top: 20,
                    right: (self.config.width as i32) - 20,
                    bottom: 20 + rect_height as i32,
                },
                default_color: Color::rgb(255, 255, 255),
                custom_glyphs: &[],
            },
        ];

        for (i, buf) in self.table_buffers.iter().enumerate() {
            let r = i / cols;
            let c = i % cols;
            let x = table_x + c as f32 * col_width;
            let y = table_y + r as f32 * row_height;
            text_areas.push(TextArea {
                buffer: buf,
                left: x + 4.0,
                top: y + 4.0,
                scale: 1.0,
                bounds: TextBounds {
                    left: x as i32,
                    top: y as i32,
                    right: (x + col_width) as i32,
                    bottom: (y + row_height) as i32,
                },
                default_color: if r == 0 {
                    Color::rgb(180, 180, 255)
                } else {
                    Color::rgb(255, 255, 255)
                },
                custom_glyphs: &[],
            });
        }

        self.text_renderer
            .prepare(
                &self.device,
                &self.queue,
                &mut self.font_system,
                &mut self.atlas,
                &self.viewport,
                text_areas,
                &mut self.swash_cache,
            )
            .unwrap();

        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    depth_slice: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: 0.05,
                            g: 0.05,
                            b: 1.00,
                            a: 1.0,
                        }),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                ..Default::default()
            });

            pass.set_pipeline(&self.rect_pipeline);
            pass.set_bind_group(0, &self.globals_bind_group, &[]);
            pass.set_vertex_buffer(0, self.rect_buffer.slice(..));
            let rect_count = 2 + 3 * 5; // 2 panels + 15 cells
            pass.draw(0..6, 0..rect_count);

            self.text_renderer
                .render(&self.atlas, &self.viewport, &mut pass)
                .unwrap();
        }

        self.queue.submit([encoder.finish()]);
        frame.present();
        self.atlas.trim();
    }
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        let window = Arc::new(
            event_loop
                .create_window(Window::default_attributes().with_title("Kaiser db"))
                .unwrap(),
        );
        self.state = Some(pollster::block_on(State::new(window)).unwrap());
        self.state.as_ref().unwrap().window.request_redraw();
    }

    fn window_event(
        &mut self,
        event_loop: &winit::event_loop::ActiveEventLoop,
        window_id: winit::window::WindowId,
        event: winit::event::WindowEvent,
    ) {
        let state = match &mut self.state {
            Some(s) => s,
            None => return,
        };

        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Resized(size) => {
                state.resize(size.width, size.height);
                state.render();
            }
            WindowEvent::RedrawRequested => state.render(),
            WindowEvent::KeyboardInput { event, .. } => {
                println!("{event:?}");
                if event.state == winit::event::ElementState::Pressed {
                    match event.logical_key {
                        winit::keyboard::Key::Character(c) => {
                            state.query.push_str(&c);
                        }
                        winit::keyboard::Key::Named(winit::keyboard::NamedKey::Space) => {
                            state.query.push(' ');
                        }
                        winit::keyboard::Key::Named(winit::keyboard::NamedKey::Backspace) => {
                            state.query.pop();
                        }
                        winit::keyboard::Key::Named(winit::keyboard::NamedKey::Enter) => {
                            state.query.push('\n');
                        }
                        _ => {}
                    }
                    state.text_buffer.set_text(
                        &mut state.font_system,
                        &state.query,
                        &Attrs::new().family(Family::SansSerif),
                        Shaping::Advanced,
                        None,
                    );
                    state.window.request_redraw();
                }
            }
            WindowEvent::ScaleFactorChanged {
                inner_size_writer, ..
            } => {
                let new_size = state.window.inner_size();
                state.resize(new_size.width, new_size.height);
                state.window.request_redraw();
            }
            _ => {}
        }
    }
}

fn main() {
    let event_loop = EventLoop::new().unwrap();
    let window_attributes = Window::default_attributes();
    let mut app = App::default();

    event_loop.run_app(&mut app);
}
