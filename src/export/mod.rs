pub mod pdf;

pub trait Exporter<Model> {
    fn export(&self, model: &Model, path: &std::path::Path) -> Result<(), String>;
}
