use winit::{application::ApplicationHandler, event_loop::EventLoop, window::Window};

#[derive(Default)]
struct App {
    window: Option<Window>,
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &winit::event_loop::ActiveEventLoop) {
        self.window = Some(
            event_loop
                .create_window(Window::default_attributes().with_title("Kaiser db"))
                .unwrap(),
        )
    }

    fn window_event(
        &mut self,
        event_loop: &winit::event_loop::ActiveEventLoop,
        window_id: winit::window::WindowId,
        event: winit::event::WindowEvent,
    ) {
    }
}

fn main() {
    let event_loop = EventLoop::new().unwrap();
    let window_attributes = Window::default_attributes();
    let mut app = App::default();

    event_loop.run_app(&mut app);
}
