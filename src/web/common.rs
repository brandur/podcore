use slog::Logger;

pub trait State {
    fn log(&self) -> &Logger;
}
